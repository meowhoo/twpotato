# Third-party notices: TwIme settings app

The TwIme settings app (`twime-settings.exe`) is MIT licensed. It is built with Tauri 2 and other open-source crates.
Most dependencies are MIT, Apache-2.0 or BSD licensed (often dual-licensed); the full list is produced by
`cargo deny list` in `src-tauri/` (policy file: `src-tauri/deny.toml`).

The following dependencies use licenses outside MIT / Apache-2.0 / BSD. Their use was accepted on 2026-10-25
(user decision). None of them is modified; their sources are available on crates.io / npm.

## Linked into the executable

| Crate | License | Reached through |
| - | - | - |
| option-ext 0.2.0 | MPL-2.0 | tauri -> dirs -> dirs-sys |
| icu_collections, icu_locale_core, icu_normalizer, icu_normalizer_data, icu_properties, icu_properties_data, icu_provider (2.4.0) | Unicode-3.0 | tauri -> url -> idna |
| litemap 0.8.3, potential_utf 0.1.6, tinystr 0.8.5, writeable 0.6.4, yoke 0.8.4, yoke-derive 0.8.4, zerofrom 0.1.8, zerofrom-derive 0.1.8, zerotrie 0.2.5, zerovec 0.11.9, zerovec-derive 0.11.7 | Unicode-3.0 | ICU4X (as above) |

MPL-2.0 source: https://crates.io/crates/option-ext . License text: https://mozilla.org/MPL/2.0/ .
Unicode License v3: https://www.unicode.org/license.txt .

## Build time only (not in the executable)

| Package | License | Use |
| - | - | - |
| cssparser 0.37.0, cssparser-macros 0.7.1, selectors 0.38.0, dtoa-short 0.3.5 | MPL-2.0 | Tauri code generation (tauri-codegen / tauri-utils) |
| foldhash 0.2.0 | Zlib | Tauri code generation |
| unicode-ident 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 | proc-macro2 |
| picocolors 1.1.1 (npm) | ISC | Vite build tool |
| source-map-js 1.2.2 (npm) | BSD-3-Clause | Vite build tool |

The web frontend ships only this project's own TypeScript; no npm package is bundled into it.
