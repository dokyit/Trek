# Background art: sources and how to make it again

Trek's background art lives in `crates/trek-assets/assets/backgrounds/` (embedded in the app) and
is made by the two scripts here. Settings › Appearance › Background shows it in four collections
(`crates/trek-app/src/backgrounds.rs`).

| Collection | Where it shows | Made by |
|---|---|---|
| Classic (Dawn, Night, Paper) | where the user puts it | `make_backgrounds.py`, procedural |
| Paintings | across the whole chat | `make_collections.py`, from the paintings below |
| ASCII (Ramp, Braille, Halftone, Dense, LED) | the sidebar only | `make_collections.py`, procedural scenes |
| Light (Streaks, Aurora, Window, Switchbacks) | across the whole chat | `make_collections.py`, procedural |

Nothing in ASCII or Light is copied or ported from another piece of art: they are drawn by
`make_collections.py` from scenes it makes itself (ridges, a lake, pines, a switchback trail,
light streaks, an aurora, a window's light). Light's Paper versions draw the same light as ember
ink with a warm-grey wash, as a print of it. The glyphs are JetBrains Mono, which Trek already
ships (`crates/trek-assets/assets/fonts/OFL-JetBrainsMono.txt`).

## The paintings

All are in the public domain; each museum publishes its image as public domain or CC0. The
files in Trek are dithered renderings in Trek's palette, not the museums' images.

| Piece | Artist | Title | Year | Collection | Image | Licence |
|---|---|---|---|---|---|---|
| Matterhorn | Albert Bierstadt | Sunrise on the Matterhorn | after 1875 | The Metropolitan Museum of Art, 10158 | https://www.metmuseum.org/art/collection/search/10158 | Public domain (Met Open Access, CC0) |
| Kauterskill | Sanford Robinson Gifford | A Gorge in the Mountains (Kauterskill Clove) | 1862 | The Metropolitan Museum of Art, 10946 | https://www.metmuseum.org/art/collection/search/10946 | Public domain (Met Open Access, CC0) |
| Oxbow | Thomas Cole | View from Mount Holyoke, Northampton, Massachusetts, after a Thunderstorm—The Oxbow | 1836 | The Metropolitan Museum of Art, 10497 | https://www.metmuseum.org/art/collection/search/10497 | Public domain (Met Open Access, CC0) |
| Merced | Albert Bierstadt | Merced River, Yosemite Valley | 1866 | The Metropolitan Museum of Art, 10150 | https://www.metmuseum.org/art/collection/search/10150 | Public domain (Met Open Access, CC0) |
| Notch | Thomas Cole | A View of the Mountain Pass Called the Notch of the White Mountains (Crawford Notch) | 1839 | National Gallery of Art (object 50727) | https://commons.wikimedia.org/wiki/File:Thomas_Cole,_A_View_of_the_Mountain_Pass_Called_the_Notch_of_the_White_Mountains_(Crawford_Notch),_1839,_NGA_50727.jpg | CC0 (NGA Open Access) |
| Green River | Thomas Moran | Green River Cliffs, Wyoming | 1881 | National Gallery of Art (object 82648) | https://commons.wikimedia.org/wiki/File:Thomas_Moran,_Green_River_Cliffs,_Wyoming,_1881,_NGA_82648.jpg | CC0 (NGA Open Access) |
| Moonlight | J. M. W. Turner | Keelmen Heaving in Coals by Moonlight | 1835 | National Gallery of Art (object 1225) | https://commons.wikimedia.org/wiki/File:Joseph_Mallord_William_Turner,_Keelmen_Heaving_in_Coals_by_Moonlight,_1835,_NGA_1225.jpg | CC0 (NGA Open Access) |
| Aosta | J. M. W. Turner | Valley of Aosta: Snowstorm, Avalanche, and Thunderstorm | 1836–37 | Art Institute of Chicago, 1947.513 (artwork 109938) | https://commons.wikimedia.org/wiki/File:Joseph_Mallord_William_Turner_-_Valley_of_Aosta,_Snowstorm,_Avalanche,_and_Thunderstorm_-_1947.513_-_Art_Institute_of_Chicago.jpg | Public domain (AIC: CC0) |

Kauterskill's hazy sun is pale (its gold is barely saturated), so the script counts it as
warm at a lower saturation than the others (`PAINTINGS`' `(gain, low, high)`), which keeps its
golden sky.

The Met's images are the "original" files its Open Access API lists for each object
(`https://collectionapi.metmuseum.org/public/collection/v1/objects/<id>`, `primaryImage`); the
others are Wikimedia Commons' 2400 px renditions of the museums' own files.

## Making it again

Download the paintings into an empty folder of their own, named as `PAINTINGS` in
`make_collections.py` names them (`matterhorn.jpg`, `kauterskill.jpg`, `oxbow.jpg`, `notch.jpg`,
`green-river.jpg`, `aosta.jpg`, `keelmen.jpg`, `merced.jpg`), then, from the repository's root:

```sh
python3 assets/backgrounds/make_backgrounds.py                      # Classic (writes here; copy to crates/trek-assets/assets/backgrounds/)
uv run --with pillow --with numpy python -I assets/backgrounds/make_collections.py <paintings-folder>
uv run --with pillow --with numpy python -I assets/backgrounds/make_collections.py <paintings-folder> paintings:oxbow ascii: light:aurora   # some
```

`make_collections.py` writes each piece's Night and Paper versions and a thumbnail of each. The
brightest pixel of a collection's Night files and the darkest of its Paper ones are written into
`Collection::extreme` in `crates/trek-app/src/backgrounds.rs`: the scrim over the chat is worked
out from them so text keeps 4.5:1, and `cargo test -p trek-app --lib backgrounds` fails, naming
the pixel, if a file asks for more than its collection's extreme.
