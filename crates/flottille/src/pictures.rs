//! The images a tool's result gives the agent: a connection's, a file `fs_read` opens, a page `web`
//! reads. Each is checked, scaled to fit under its caps, stored as a message's picture is (a file
//! of the agent's folder of images, never base64 in the store or an event) and handed to the call
//! in progress, which sends it as MCP `image` content (`docs/tools.md`, "Images in a result").

use crate::daemon::Daemon;

/// The most images of one result that reach the agent, the biggest one of them and all of them
/// together, in bytes.
pub const MAX_PICTURES: usize = 4;
pub const MAX_PICTURE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_PICTURES_BYTES: usize = 5 * 1024 * 1024;

/// Whether the agent's provider reads an image in the result of a tool.
fn reads_tool_images(daemon: &Daemon, agent: &str) -> bool {
    daemon
        .read(|store| store.agent(agent))
        .ok()
        .flatten()
        .and_then(|agent| crate::providers::of(&agent).ok())
        .is_some_and(|provider| provider.takes_tool_images)
}

/// What an image became when it was taken.
pub struct Taken {
    /// What was stored and goes to the agent.
    pub mime: &'static str,
    pub bytes: usize,
    /// Its size in pixels, from its header.
    pub size: Option<(u32, u32)>,
    /// What it was before it was scaled, when it was.
    pub scaled_from: Option<Scaled>,
}

/// An image as it was before it was scaled down.
pub struct Scaled {
    pub mime: String,
    pub bytes: usize,
    pub size: Option<(u32, u32)>,
}

/// The images of one result, taken or left out, as the agent's folder and the caps say.
pub struct Pictures<'a> {
    daemon: &'a Daemon,
    agent: &'a str,
    /// Whether any image may be taken: not for an error, nor for an agent that cannot read one.
    refused: Option<&'static str>,
    taken: usize,
    bytes: usize,
}

impl<'a> Pictures<'a> {
    /// The images of the result of a call of `agent`, an error's when `failed`.
    pub fn of_result(daemon: &'a Daemon, agent: &'a str, failed: bool) -> Pictures<'a> {
        Pictures {
            daemon,
            agent,
            refused: match failed {
                true => Some("an error carries no image"),
                false => (!reads_tool_images(daemon, agent))
                    .then_some("this agent's provider reads no image in the result of a tool"),
            },
            taken: 0,
            bytes: 0,
        }
    }

    /// Takes an image of type `mime`, its bytes given by `bytes` once its type is known to be
    /// one that is taken. The image is stored and given to the call in progress, or the reason
    /// it is left out is returned.
    pub fn take(
        &mut self,
        mime: &str,
        bytes: impl FnOnce() -> Result<Vec<u8>, String>,
    ) -> Result<Taken, String> {
        if let Some(why) = self.refused {
            return Err(why.to_owned());
        }
        let Some((_, extension)) = crate::deliveries::IMAGE_TYPES
            .iter()
            .find(|(m, _)| *m == mime)
        else {
            return Err("only png, jpeg, gif and webp are taken".to_owned());
        };
        if self.taken >= MAX_PICTURES {
            return Err(format!("a result carries {MAX_PICTURES} images at most"));
        }
        let bytes = bytes()?;
        if bytes.is_empty() {
            return Err("it is empty".to_owned());
        }
        // The type a server says is not checked by the provider's API, which refuses an image
        // whose bytes are not its type and keeps it in the history, breaking every later turn.
        if !flottille_core::images::is_picture(mime, &bytes) {
            let kind = mime.strip_prefix("image/").unwrap_or(mime);
            return Err(format!("its bytes are not those of a {kind}"));
        }
        // Scaled before the caps are looked at, as the pictures of a message are before they are
        // stored: the answer's own size limit is what bounds what is decoded.
        let before = Scaled {
            mime: mime.to_owned(),
            bytes: bytes.len(),
            size: flottille_core::images::dimensions(mime, &bytes),
        };
        // What cannot be decoded, or is too large to be, is left out: sent as it is, the provider
        // would refuse it, and it would stay in the history.
        let fitted = flottille_core::images::fit_under(mime, &bytes, MAX_PICTURE_BYTES)?;
        let (bytes, extension, mime, size, scaled_from) = match fitted {
            Some(fitted) => {
                // A PNG that became a JPEG is stored as one: its name says what it is.
                let extension = if fitted.mime == "image/jpeg" {
                    "jpg"
                } else {
                    "png"
                };
                let size = Some((fitted.width, fitted.height));
                (fitted.bytes, extension, fitted.mime, size, Some(before))
            }
            None => {
                let mime = crate::deliveries::IMAGE_TYPES
                    .iter()
                    .find(|(m, _)| *m == mime)
                    .map_or("image/png", |(m, _)| *m);
                (bytes, *extension, mime, before.size, None)
            }
        };
        if bytes.len() > MAX_PICTURE_BYTES {
            return Err(format!("over {} MiB", MAX_PICTURE_BYTES >> 20));
        }
        if self.bytes + bytes.len() > MAX_PICTURES_BYTES {
            return Err(format!(
                "over {} MiB for the images of one result",
                MAX_PICTURES_BYTES >> 20
            ));
        }
        let Some(name) = self.store(extension, &bytes) else {
            return Err("it could not be stored".to_owned());
        };
        crate::mcp::picture_served(&name);
        self.taken += 1;
        self.bytes += bytes.len();
        Ok(Taken {
            mime,
            bytes: bytes.len(),
            size,
            scaled_from,
        })
    }

    fn store(&self, extension: &str, bytes: &[u8]) -> Option<String> {
        let folder = self.daemon.paths.images(self.agent);
        flottille_core::paths::ensure_private_dir(&folder).ok()?;
        let name = format!("{}.{extension}", flottille_core::agent::new_id().ok()?);
        std::fs::write(folder.join(&name), bytes).ok()?;
        Some(name)
    }
}

/// A size as the line of an image says it.
pub fn size_said(bytes: usize) -> String {
    match bytes {
        1_000_000.. => format!("{:.1} MB", bytes as f64 / 1_000_000.0),
        1_000.. => format!("{} KB", bytes / 1000),
        _ => format!("{bytes} bytes"),
    }
}

/// The line that names an image a tool read (`fs_read`, `web`): what it is, its size in pixels,
/// and that it is attached, scaled when it was, or why it is left out. `named` says where it is
/// from and what it weighed, `size` its size in pixels as it was.
pub fn described(named: &str, size: Option<(u32, u32)>, taken: &Result<Taken, String>) -> String {
    let pixels = |size: Option<(u32, u32)>| {
        size.map(|(width, height)| format!(", {width}×{height}"))
            .unwrap_or_default()
    };
    match taken {
        Err(why) => format!("[image: {named}, left out: {why}]"),
        Ok(Taken {
            scaled_from: Some(_),
            mime,
            bytes,
            size: after,
        }) => format!(
            "[image: {named}{}; scaled to {} {}{}, is attached]",
            pixels(size),
            size_said(*bytes),
            kind_said(mime),
            pixels(*after)
        ),
        Ok(_) => format!("[image: {named}{}, is attached]", pixels(size)),
    }
}

/// The name of a type as a line says it: `image/png` is PNG.
pub fn kind_said(mime: &str) -> String {
    mime.strip_prefix("image/").unwrap_or(mime).to_uppercase()
}

/// The type of an image by its first bytes, whatever its file is named: `None` for anything else.
pub fn sniffed(start: &[u8]) -> Option<&'static str> {
    crate::deliveries::IMAGE_TYPES
        .iter()
        .map(|(mime, _)| *mime)
        .find(|mime| flottille_core::images::is_picture(mime, start))
}
