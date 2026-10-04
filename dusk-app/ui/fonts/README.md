# Bundled fonts

Embedded in `dusk.exe` at build time through the `import` lines in `../theme.slint`. Both families are under the SIL Open Font License 1.1 (no reserved font names), which allows embedding and shipping them with Dusk as long as each copy carries the copyright notice and the license. Keep `Inter/LICENSE.txt` and `JetBrainsMono/OFL.txt` next to the fonts, and ship both with the installer.

| File | Bytes | SHA-256 |
|---|---|---|
| `Inter/Inter-Regular.ttf` | 411 640 | `40d692fce188e4471e2b3cba937be967878f631ad3ebbbdcd587687c7ebe0c82` |
| `Inter/Inter-Medium.ttf` | 417 300 | `97ad806f526e41546d46365bb3a393145f75b7b1568913db74549ad8b8dba872` |
| `Inter/Inter-SemiBold.ttf` | 419 744 | `78a843fade9d4612a5567302fb595b56976eb5fcebf4fea5a5912d638bafcde3` |
| `JetBrainsMono/JetBrainsMono-Regular.ttf` | 273 900 | `a0bf60ef0f83c5ed4d7a75d45838548b1f6873372dfac88f71804491898d138f` |

Source, unmodified:

- Inter v4.1, `extras/ttf/` and `LICENSE.txt` from <https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip> (SHA-256 `9883fdd4a49d4fb66bd8177ba6625ef9a64aa45899767dde3d36aa425756b11e`).
- JetBrains Mono v2.304, `fonts/ttf/` and `OFL.txt` from <https://github.com/JetBrains/JetBrainsMono/releases/download/v2.304/JetBrainsMono-2.304.zip> (SHA-256 `6f6376c6ed2960ea8a963cd7387ec9d76e3f629125bc33d1fdcd7eb7012f7bbf`).

Neither release publishes checksums; the sums above are of the files as downloaded on 2026-10-04.

Why static Inter files and not `InterVariable.ttf`: the variable font's family name is "Inter Variable", while the three static files share the typographic family "Inter", so Slint's font matching picks a real face for each weight Dusk uses (400, 500, 600; see `docs/THEME.md`).
