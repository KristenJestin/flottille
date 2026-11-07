# The brand of Flottille

The mark is the window's own corner: two sails over one hull, a flotilla reduced to a lead and the
agent it started. The sails are filled and the hull is one curved stroke, so the drawing holds at
16 pixels and in the 20 pixel tile of the window's top bar. The SVGs of this folder are the one
source of the drawing; the PNGs are rendered from them by `tools/brand.sh` and committed. To change
the brand, edit the SVGs and run it again.

| Source | For |
| --- | --- |
| `mark.svg` | the mark alone, for a dark background (the default) |
| `mark-on-light.svg` | the mark alone, for a light background |
| `icon.svg` | the icon: the mark on the window's dark ground, in a rounded square |
| `icon-full.svg` | the full icon, as the window's corner draws it: an off-white square, the mark in ink |
| `favicon.svg` | the favicon, the full icon with less margin, drawn for 16 to 64 pixels |
| `screen.svg` | the scene, without words, on a transparent ground and held to the right, for pages that put a title beside it: the window's Agents column (the lead and the three sub-agents it started) and a request the lead forwarded, waiting for the user |
| `social-preview.svg` | the social preview of the repository (1280 by 640): the icon, the name, the tagline, and the window below them, zoomed out: its top bar, a session's conversation (a request, the lead's answer, the sub-agents it launched, a report folded) and the Agents rail, cut by the lower edge |
| `social-preview-fr.svg` | the same with the French tagline, for French pages |

The scene is drawn, not captured, with the window's palette and words, and invented projects only.
The card says what the window says for a request a sub-agent asked and its lead passed on: the
call, the lead's sentence ("The conversation forwards it: …") and who asked and who forwarded it.
A sub-agent never asks the user directly and never starts an agent of its own
(`docs/design.md` §5.1 and §7), so the scene shows neither.

The colours are the window's (`crates/flottille-ui/src/theme.rs`): ink `#ececec` on the dark
ground `#0b0b0b` and `#141414`, `#141414` on a light one, amber `#f5a524` for what waits for the
user, blue `#7aa7ff` for a running agent, green `#4ade80` for one that reported.

## What is generated

`png/`: the icon at 128, 256, 512 and 1024 pixels (`icon-*.png`), the full icon at 512, the
favicon at 16, 32 and 64, the marks at 512, `screen.png` (1600 by 1000), `social-preview.png`
and `social-preview-fr.png` (1280 by 640). `tools/brand.sh` needs `rsvg-convert` (librsvg) and
draws the text with the fonts of `fonts/` only, through a fontconfig made for the run.

`png/social-preview.png` is uploaded by hand in the repository's settings on GitHub (Settings,
General, Social preview), as GitHub offers no other way.
