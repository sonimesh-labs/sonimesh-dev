# Desktop UI viewport and layout check - 2026-08-16

> Status: programmatic layout audit of the preview desktop receiver UI in a
> real browser. It checks overflow, clipping, and interactive-element overlap
> across viewport widths and busy preview states; it does not assess visual
> aesthetics, color, or pixel-perfect rendering, and it is not an audio or
> transport measurement.
> Updated: 2026-08-17

## Purpose

The desktop control UI must stay readable and usable across the supported
viewport range without horizontal page overflow, clipped text, or overlapping
controls. This check drives the Vue preview adapter in Chromium through the
initial and busy states at desktop, phone, and narrow-phone widths and audits
the rendered layout programmatically so the result is reproducible without a
human visual pass.

## Environment

| Field | Observed value |
| --- | --- |
| Local run date | 2026-08-16, Asia/Shanghai |
| Host | Windows 11 reference host |
| App | `apps/desktop-ui` 0.1.0 (Vue 3.5.40, Vite 8.1.5 dev server) |
| Browser | Chromium via Playwright |
| Preview state | `PreviewDesktopBridge` initial snapshot and a busy simulated state |
| Viewports | 1440x900, 390x844, 320x568 (CSS pixels) |

## Method

The dev server ran on `127.0.0.1:5173`. For each viewport width and state the
page was audited with one script that reports:

- horizontal page overflow: `documentElement.scrollWidth` exceeding the
  viewport width;
- element clipping: any visible element whose `scrollWidth` exceeds
  `clientWidth` while its computed horizontal `overflow` is not `hidden`/`clip`;
- viewport protrusion: any visible element whose bounding box extends beyond
  the viewport edges;
- interactive overlap: any two visible `button`/`a`/`input`/`select` elements
  whose bounding boxes overlap by more than 40% of the smaller element's area
  (nested elements excluded).

The busy state was produced through the preview bridge itself: reset preview,
start listening, two metrics bursts, connection loss (reconnecting with a 30 s
countdown), then nine diagnostic exports (retained under the fixed eight-receipt
bound).

## Results

| Viewport | State | Page overflow | Clipped text | Protruding elements | Interactive overlap |
| --- | --- | ---: | ---: | ---: | ---: |
| 1440x900 | initial `ready` | none | none | none | none |
| 1440x900 | `reconnecting` + metrics + exports | none | none | none | none |
| 390x844 | initial `ready` | none | none | none | none |
| 320x568 | initial `ready` | none | none | none | none |
| 320x568 | `reconnecting` + metrics + exports | none | none | none | none |

The interaction sequence behaved as specified: start listening reached
`streaming`, the metrics counters advanced and stayed bounded (for example the
reply log capped at six, the diagnostic receipts at eight), and connection loss
showed `reconnecting` with a `30s remaining` window. No layout defect was
observed in any state or width.

Full-page screenshots were saved as local artifacts under `output/playwright/`
(`sonimesh-desktop-1440.png`, `sonimesh-mobile-390.png`,
`sonimesh-mobile-320.png`); `output/` is ignored by git, so the screenshots are
not part of the repository.

## Limitations

- The audit is programmatic: it detects geometry defects but not aesthetic
  issues, contrast, or font-rendering problems, which still warrant a human
  visual review before release.
- It exercises the preview adapter only, not the future Tauri command surface;
  the desktop shell remains out of scope for this check.
- Browser layout engines can differ; this run reflects the reference host's
  Chromium only.

## Follow-up: pixel-level sanity check of screenshots (2026-08-17)

> Status: supplementary automated check on the saved artifacts. It confirms the
> screenshots contain rendered UI content; it does not replace a human
> aesthetic/contrast review.

| Artifact | Size (px) | Avg RGB | Dark px | Light px | Accent px |
| --- | --- | --- | ---: | ---: | ---: |
| `sonimesh-desktop-1440.png` | 1425x2361 | (203,208,206) | 14.6% | 79.2% | 2.9% |
| `sonimesh-mobile-390.png` | 375x4628 | (203,208,206) | 15.7% | 78.5% | 2.5% |
| `sonimesh-mobile-320.png` | 320x4764 | (210,215,213) | 10.0% | 83.1% | 4.2% |

The three full-page artifacts are non-blank with the expected light-background /
dark-text distribution and small accent-color share, consistent with the
programmatic layout audit above. A true visual pass (aesthetics, contrast,
font rendering) remains open: the current agent tool environment rejected image
input, so no model-based visual review was performed.
