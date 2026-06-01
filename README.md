# decodingus-shared

Rust crates shared between the **DecodingUs** AppView (server) and the
**Navigator** edge desktop app. Extracted from the `decodingus` repo so both
apps depend on one copy and fixes flow both ways.

| Crate | Contents | Navigator reuse |
|---|---|---|
| `du-domain` | Pure domain types, enums, IDs, JSONB payload shapes. Zero IO. | **Full** |
| `du-atproto` | AT Protocol identity/crypto: DID + AT-URI parsing, `did:key` Ed25519 verification, DID/handle/PDS resolution, and the OAuth client crypto (PKCE, DPoP, ES256 JOSE, client/auth-server metadata, request builders — both confidential `private_key_jwt` and **public-client PKCE-only** paths). | **Partial** — crypto/resolution + public-client builders; not the server's confidential-client wiring (which lives in `decodingus`). |
| `du-bio` | Genomics coordinate math + text parsing: VCF variant ingest, BED callable-loci, UCSC-chain liftover, YBrowse GRCh38→build liftover. No BAM/CRAM / variant calling (that's Navigator-side). | **Full** |

> The Navigator-side haploid variant caller stays a Navigator-only crate by
> decision; `du-bio` remains coordinate math + text parsing.

## Build

```sh
cargo test            # builds + tests all three crates (no DB needed)
```

## Consuming

Both repos depend on these crates. During local co-development, `decodingus`
references them by **path** (`../../decodingus-shared/crates/*`); once this repo
has a remote, switch consumers to **git deps** pinned to a tag/rev:

```toml
du-domain  = { git = "https://github.com/decodingus/decodingus-shared", tag = "v0.1.0" }
du-atproto = { git = "https://github.com/decodingus/decodingus-shared", tag = "v0.1.0" }
du-bio     = { git = "https://github.com/decodingus/decodingus-shared", tag = "v0.1.0" }
```
