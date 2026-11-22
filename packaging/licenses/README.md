# Rust dependency license inventory

`RUST-DEPENDENCIES.md` preserves the license expressions and license, authors,
copyright and notice files from every registry package in Cargo.lock, including
build/test and other-platform packages. Native libraries and separately installed
media tools are outside this inventory.

Regenerate offline from repository sources and a populated local registry:

```sh
python3 packaging/licenses/generate.py
python3 packaging/licenses/generate.py --check
```

The `upstream` supplements cover release crates whose archives omit top-level
license texts. `provenance.json` records the exact official upstream commit,
retrieval URL and SHA-256 for each file. Commits match each crate archive's
`.cargo_vcs_info.json`; Lofty 0.25.4 and r-efi 5.3.0/6.0.0 also match official
release tags. lofty_attr and ogg_pager use their own published-crate commits,
which differ from similarly named repository release tags. r-efi publishes its
license grants and authorship in AUTHORS; preserve that file in full.

The companion dependency source archive carries the complete unmodified crate
source directories from `cargo vendor --locked --offline --versioned-dirs`,
Cargo.lock, these notices and the offline source replacement config. Extract it
and copy both `vendor/` and `.cargo/config.toml` into the matching Gnune source
root to build without downloading crates. Rust toolchains and native
Fedora development packages are still required. The registry archive checksums
are preserved in each vendor directory's `.cargo-checksum.json`.

These files retain upstream license alternatives; no license choice or new terms
are imposed by this inventory. Per-file terms remain in the vendored sources.
