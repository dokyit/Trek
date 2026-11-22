"""The home page's paper: a seamless tile of paper tooth and soft unevenness, with no
direction to it (the line is the only thing on the page that goes anywhere).

    uv run --with pillow --with numpy python -I script/site/make_paper.py && cwebp -q 80 -m 6 site/assets/img/home/paper.png -o site/assets/img/home/paper.webp

Deterministic: the same seed makes the same paper."""
import pathlib
import numpy as np
from PIL import Image

OUT = pathlib.Path(__file__).resolve().parents[2] / "site" / "assets" / "img" / "home"
rng = np.random.default_rng(2211)


def periodic_noise(n, f, power):
    white = rng.standard_normal((n, n))
    F = np.fft.fft2(white)
    k = np.sqrt((np.fft.fftfreq(n)[:, None] * n) ** 2 + (np.fft.fftfreq(n)[None, :] * n) ** 2) / f + 1e-6
    F *= 1.0 / (1.0 + k ** power)
    F[0, 0] = 0
    out = np.real(np.fft.ifft2(F))
    return (out - out.mean()) / out.std()


def paper(n=512):
    tooth = periodic_noise(n, 160.0, 1.2)
    cloud = periodic_noise(n, 5.0, 2.4)
    fibre = periodic_noise(n, 40.0, 3.0)
    v = np.tanh((0.5 * tooth + 0.35 * cloud + 0.25 * fibre) * 0.7)
    g = (128 + 22 * v).clip(0, 255).astype(np.uint8)
    Image.fromarray(g, "L").save(OUT / "paper.png")


if __name__ == "__main__":
    OUT.mkdir(parents=True, exist_ok=True)
    paper()
    print("wrote", OUT / "paper.png")
