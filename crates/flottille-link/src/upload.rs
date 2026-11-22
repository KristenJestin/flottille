//! The pictures and pasted texts of a message sent to another machine in chunks (`agent.upload`,
//! `docs/remote.md`, "Pictures sent to another machine"): a client whose message, signed, would not
//! fit one frame of the link sends each file in chunks first, each one signed request, then the
//! message naming the upload. The window and the phone share this, so that they chunk alike.

use serde_json::{Value, json};

use crate::link::FRAME_LIMIT;

/// The most of a file one chunk carries: in base64, signed, it stays within a frame of the link.
pub const UPLOAD_CHUNK: usize = 512 * 1024;

/// What a signed request holds beside its parameters, escaped once more: its id, its machine, the
/// device's key, the nonce, the time, the signature, with room to spare.
const SIGNED_ROOM: usize = 4096;

/// A file of a message: a picture, or a long pasted text.
#[derive(Clone, Copy, Debug)]
pub enum Attached<'a> {
    Picture { mime: &'a str, bytes: &'a [u8] },
    Text(&'a str),
}

/// What a file adds to a message beside its bytes: its keys, its quotes escaped twice, its name.
const FILE_ROOM: usize = 512;

/// The length of `value` as a signed request carries it: serialised, then escaped a second time as
/// the string it becomes.
fn twice(value: &Value) -> usize {
    let once = value.to_string();
    serde_json::to_string(&once)
        .expect("a string is JSON")
        .len()
}

/// The length of `text` escaped for JSON twice, counted without building it.
fn escaped_twice(text: &str) -> usize {
    text.chars()
        .map(|c| match c {
            '"' | '\\' => 4,
            '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 3,
            c if (c as u32) < 0x20 => 7,
            c => c.len_utf8(),
        })
        .sum()
}

/// Whether `method` with `params`, signed, fits in one frame of the link: signed, the parameters
/// travel as a string, escaped a second time.
pub fn fits(method: &str, params: &Value) -> bool {
    twice(params) + method.len() + SIGNED_ROOM <= FRAME_LIMIT
}

/// Whether `method` with `words` and `files` inline would, signed, fit in one frame of the link:
/// judged from the files' sizes alone, without encoding any of them, so that a message of large
/// pictures costs nothing to refuse inline. It never says yes to a message that would not fit.
pub fn fits_with(method: &str, words: &Value, files: &[Attached]) -> bool {
    let mut size = twice(words) + method.len() + SIGNED_ROOM;
    for file in files {
        size += FILE_ROOM
            + match *file {
                // Base64 has nothing to escape.
                Attached::Picture { mime, bytes } => {
                    escaped_twice(mime) + bytes.len().div_ceil(3) * 4
                }
                Attached::Text(text) => escaped_twice(text),
            };
        if size > FRAME_LIMIT {
            return false;
        }
    }
    size <= FRAME_LIMIT
}

/// A new upload's name: 16 random bytes in hex, which no other upload of the device has.
pub fn upload_name() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("no random bytes for an upload: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The parameters of each `agent.upload` that sends `files` as the upload `upload`, in order:
/// the pictures numbered from 1 among the pictures, the texts among the texts.
pub fn upload_chunks<'a>(
    upload: &'a str,
    files: &'a [Attached<'a>],
) -> impl Iterator<Item = Value> + 'a {
    use base64::Engine as _;
    let (mut pictures, mut texts) = (0u32, 0u32);
    files.iter().flat_map(move |file| {
        let (kind, mime, bytes, number) = match *file {
            Attached::Picture { mime, bytes } => {
                pictures += 1;
                ("image", Some(mime), bytes, pictures)
            }
            Attached::Text(text) => {
                texts += 1;
                ("text", None, text.as_bytes(), texts)
            }
        };
        bytes
            .chunks(UPLOAD_CHUNK)
            .enumerate()
            .map(move |(at, part)| {
                let mut params = json!({
                    "upload": upload,
                    "file": number,
                    "kind": kind,
                    "size": bytes.len(),
                    "offset": at * UPLOAD_CHUNK,
                    "data": base64::engine::general_purpose::STANDARD.encode(part),
                });
                if let Some(mime) = mime {
                    params["mime"] = json!(mime);
                }
                params
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chunk_signed_fits_a_frame() {
        let files = [Attached::Picture {
            mime: "image/png",
            bytes: &[7; UPLOAD_CHUNK],
        }];
        let chunk = upload_chunks("u", &files).next().unwrap();
        assert!(fits("agent.upload", &chunk));
    }

    #[test]
    fn files_go_in_order_each_numbered_among_its_kind() {
        let big = vec![1u8; UPLOAD_CHUNK + 3];
        let files = [
            Attached::Picture {
                mime: "image/png",
                bytes: &big,
            },
            Attached::Text("notes"),
            Attached::Picture {
                mime: "image/jpeg",
                bytes: &[2; 4],
            },
        ];
        let chunks: Vec<Value> = upload_chunks("u", &files).collect();
        let seen: Vec<_> = chunks
            .iter()
            .map(|c| (c["kind"].clone(), c["file"].clone(), c["offset"].clone()))
            .collect();
        assert_eq!(
            seen,
            [
                (json!("image"), json!(1), json!(0)),
                (json!("image"), json!(1), json!(UPLOAD_CHUNK)),
                (json!("text"), json!(1), json!(0)),
                (json!("image"), json!(2), json!(0)),
            ]
        );
        assert_eq!(chunks[1]["size"], UPLOAD_CHUNK + 3);
        assert_eq!(chunks[3]["mime"], "image/jpeg");
        assert!(chunks[2].get("mime").is_none());
    }

    #[test]
    fn a_message_past_a_frame_does_not_fit() {
        assert!(fits("agent.send", &json!({"message": "hello"})));
        let quotes = "\"".repeat(FRAME_LIMIT / 2);
        // Escaped twice, the quotes take four times their room.
        assert!(!fits("agent.send", &json!({"message": quotes})));
    }

    /// What this thread holds of the heap, and the most it held since the last reset: a message of
    /// large pictures must be judged and chunked without ever holding them encoded at once.
    mod heap {
        use std::alloc::{GlobalAlloc, Layout, System};
        use std::cell::Cell;

        thread_local! {
            static LIVE: Cell<usize> = const { Cell::new(0) };
            static PEAK: Cell<usize> = const { Cell::new(0) };
        }

        pub struct Counting;

        unsafe impl GlobalAlloc for Counting {
            unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
                grow(layout.size());
                unsafe { System.alloc(layout) }
            }
            unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
                let _ = LIVE.try_with(|live| live.set(live.get().saturating_sub(layout.size())));
                unsafe { System.dealloc(ptr, layout) }
            }
            unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new: usize) -> *mut u8 {
                let _ = LIVE.try_with(|live| live.set(live.get().saturating_sub(layout.size())));
                grow(new);
                unsafe { System.realloc(ptr, layout, new) }
            }
        }

        fn grow(size: usize) {
            let _ = LIVE.try_with(|live| {
                live.set(live.get() + size);
                let _ = PEAK.try_with(|peak| peak.set(peak.get().max(live.get())));
            });
        }

        /// The most `work` held above what was held when it began.
        pub fn peak_of<T>(work: impl FnOnce() -> T) -> (T, usize) {
            let before = LIVE.get();
            PEAK.set(before);
            let done = work();
            (done, PEAK.get() - before)
        }
    }

    #[global_allocator]
    static HEAP: heap::Counting = heap::Counting;

    fn twenty_photos() -> Vec<Vec<u8>> {
        (0..20u8)
            .map(|n| {
                (0..5 * 1024 * 1024)
                    .map(|at| (at % 251) as u8 ^ n)
                    .collect()
            })
            .collect()
    }

    #[test]
    fn twenty_large_pictures_are_judged_without_being_encoded() {
        let photos = twenty_photos();
        let files: Vec<Attached> = photos
            .iter()
            .map(|bytes| Attached::Picture {
                mime: "image/jpeg",
                bytes,
            })
            .collect();
        let (fit, peak) =
            heap::peak_of(|| fits_with("agent.send", &json!({"message": "hi"}), &files));
        assert!(!fit);
        assert!(peak < 64 * 1024, "{peak} bytes held to judge 100 MiB");
    }

    #[test]
    fn twenty_large_pictures_go_in_chunks_one_at_a_time() {
        let photos = twenty_photos();
        let files: Vec<Attached> = photos
            .iter()
            .map(|bytes| Attached::Picture {
                mime: "image/jpeg",
                bytes,
            })
            .collect();
        let (sent, peak) = heap::peak_of(|| {
            let mut sent = 0;
            for chunk in upload_chunks("u", &files) {
                sent += chunk["data"].as_str().unwrap().len();
            }
            sent
        });
        assert!(sent > 100 * 1024 * 1024);
        assert!(peak < 4 * 1024 * 1024, "{peak} bytes held at most at once");
    }

    #[test]
    fn a_message_judged_by_its_files_fits_only_when_it_would() {
        let words = json!({"message": "hi"});
        let small = [Attached::Picture {
            mime: "image/png",
            bytes: &[1; 1000],
        }];
        assert!(fits_with("agent.send", &words, &small));
        // The judgement is the signed message's: what it says is never smaller than what is sent.
        for size in [300_000, 600_000, 700_000, 800_000] {
            let bytes = vec![9u8; size];
            let files = [Attached::Picture {
                mime: "image/png",
                bytes: &bytes,
            }];
            let mut inline = words.clone();
            let data = {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD.encode(&bytes)
            };
            inline["images"] = json!([{"mime": "image/png", "data": data}]);
            if fits_with("agent.send", &words, &files) {
                assert!(fits("agent.send", &inline), "{size} bytes");
            }
            assert_eq!(fits_with("agent.send", &words, &files), size <= 700_000);
        }
        let quotes = "\"".repeat(FRAME_LIMIT / 4);
        assert!(!fits_with("agent.send", &words, &[Attached::Text(&quotes)]));
        assert!(fits_with(
            "agent.send",
            &words,
            &[Attached::Text(&"a".repeat(FRAME_LIMIT / 2))]
        ));
    }

    #[test]
    fn two_uploads_are_named_apart() {
        let (one, two) = (upload_name().unwrap(), upload_name().unwrap());
        assert_eq!(one.len(), 32);
        assert_ne!(one, two);
    }
}
