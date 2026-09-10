# Hashgram brand

Everything here is generated from one geometric source
(`web/scripts/brand-lib.ts`) by `pnpm run build:brand`, and published at
`https://hashgram.io/brand` for anyone to download (`hashgram-brand.zip`).

## The mark

A heavy `#` — the hash sign — drawn as four 12-unit strokes on a 64-unit
grid. The four intersections are knocked out: the negative-space squares read
as blocks, the strokes as the chain that links them. One colour, no
gradients, no outline.

The geometry is pixel-aligned at 16 px (4 grid units per pixel: 3 px strokes,
2 px holes, 2 px gaps), so the favicon stays crisp without a separate
small-size drawing.

```
logo.svg                   white mark, transparent (default on dark)
logo-white.svg             same
logo-black.svg             black mark, transparent (for light surfaces)
logo-white-on-black.svg    mark with solid black square
logo-black-on-white.svg    mark with solid white square
logo-<variant>-<size>.png  16 · 32 · 48 · 64 · 128 · 180 · 192 · 256 · 512 · 1024
```

## The wordmark

Mark + `hashgram` in Inter 700, lower case, letter-spacing −2 %, converted to
outlines (no font needed to render). Cap height of the word = 78 % of the mark
height; gap between mark and word = 32 % of the mark height.

```
wordmark.svg               white, transparent
wordmark-<variant>.svg
wordmark-<variant>-<h>.png 64 · 128 · 256 · 512 px tall
```

## Colour

Strict monochrome. Exactly seven values, nothing else:

| Name    | Hex       | Use                          |
| ------- | --------- | ---------------------------- |
| Black   | `#000000` | background                   |
| Ink 950 | `#0D0D0D` | cards, secondary surfaces    |
| Ink 900 | `#1A1A1A` | borders, dividers            |
| Ink 800 | `#262626` | inputs, hover borders        |
| Ink 700 | `#404040` | disabled, tertiary strokes   |
| Ink 500 | `#808080` | secondary text               |
| White   | `#FFFFFF` | primary text, the mark       |

The mark itself is only ever pure white or pure black. Never recolour it,
never add a shadow, never place it on a photograph without a solid black or
white plate behind it.

## Clear space and minimum size

- Clear space around the mark: at least **¼ of the mark's height** on every
  side (16 units on the 64 grid). Around the wordmark: at least the height of
  the letter `h`.
- Minimum size: mark **16 px** on screen / **6 mm** in print; wordmark
  **20 px** tall on screen / **8 mm** in print.
- Do not rotate, skew, outline, or animate the mark beyond a fade.

## Files for apps

- `favicon.svg`, `favicon-16.png`, `favicon-32.png`, `favicon-48.png` — white
  mark on a black rounded square.
- `apple-touch-icon.png` (180), `icon-192.png`, `icon-512.png`,
  `icon-maskable-512.png` — for PWA / mobile.
- `palette.json` — the seven colours as data.

## Licence

The Hashgram mark and wordmark identify the Hashgram network and its official
site. You may use them unmodified to refer to Hashgram (articles, wallets,
explorers, integrations, node dashboards). Do not use them to imply
endorsement, and do not register them as your own mark.
