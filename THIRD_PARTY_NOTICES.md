# Third-party notices

## EiSen
https://github.com/thucne/EiSen (commit ff1128b)

Ported to Rust:
- `app/src/lib/overlay/geometry.ts` → `crates/snapling-core/src/selection.rs`
- pointer/keyboard logic of `app/src/routes/overlay/+page.svelte` → `crates/snapling-core/src/selection.rs`, `crates/snapling-win/src/overlay.rs`
- `app/src/lib/editor/elements.ts`, `store.ts`, `exportPipeline.ts` (arrow head geometry, step badge, snapshot undo/redo) → `crates/snapling-core/src/annotate.rs`

```
MIT License

Copyright (c) 2026 Tran Trong Thuc

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Ubuntu Light font

Embedded via `epaint_default_fonts` and used to render annotation text.
Licensed under the Ubuntu Font Licence 1.0: https://ubuntu.com/legal/font-licence

## OPUS-MT translation models

Not bundled; downloaded on the user's request from https://huggingface.co/Xenova/opus-mt-ru-en
and https://huggingface.co/Xenova/opus-mt-en-ru (ONNX exports by Xenova of Helsinki-NLP/opus-mt-ru-en
and Helsinki-NLP/opus-mt-en-ru). © Language Technology Research Group at the University of Helsinki,
licensed under CC BY 4.0: https://creativecommons.org/licenses/by/4.0/

## ONNX Runtime

Statically linked through the `ort` crate. © Microsoft Corporation, MIT License.

## Inter, JetBrains Mono

`assets/fonts`: Inter (© The Inter Project Authors) and JetBrains Mono (© The JetBrains Mono
Project Authors), SIL Open Font License 1.1 — full texts in `assets/fonts/*-OFL.txt`.
