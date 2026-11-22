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

/// Whether `method` with `params`, signed, fits in one frame of the link: signed, the parameters
/// travel as a string, escaped a second time.
pub fn fits(method: &str, params: &Value) -> bool {
    let once = params.to_string();
    let twice = serde_json::to_string(&once)
        .expect("a string is JSON")
        .len();
    twice + method.len() + SIGNED_ROOM <= FRAME_LIMIT
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

    #[test]
    fn two_uploads_are_named_apart() {
        let (one, two) = (upload_name().unwrap(), upload_name().unwrap());
        assert_eq!(one.len(), 32);
        assert_ne!(one, two);
    }
}
