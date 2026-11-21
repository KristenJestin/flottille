# Flottille's sounds

Two short sounds the daemon plays for a notification that rings (`docs/design.md`, §11):

- `needs-you.wav`: what needs the user, a request or a question (two notes going up);
- `tell.wav`: the rest, a report, a failure, a pull request, a ticket (one note).

They are made by [`make.py`](make.py) from sine waves alone, nothing sampled: they are Flottille's
own, under the licence of the repository (AGPL-3.0-only). Mono, 16 bits, 22.05 kHz, under half a
second, and quiet. The daemon embeds them (`crates/flottille/src/sounds.rs`).
