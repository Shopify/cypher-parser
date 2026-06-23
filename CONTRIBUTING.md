# Contributing

Thanks for your interest in contributing to `cypher-parser`!

## Contributor License Agreement (CLA)

All contributors must sign the Shopify Contributor License Agreement. A bot will automatically
prompt you on your first pull request — follow its instructions to sign. We can only accept
contributions once the CLA has been signed.

## Development

This is a standard Cargo crate with no external dependencies.

- Build: `cargo build`
- Test: `cargo test`
- Lint: `cargo clippy --all-targets`
- Format: `cargo fmt`

Please make sure `cargo fmt --check`, `cargo clippy --all-targets`, and `cargo test` all pass before
opening a pull request. CI runs these on every PR.

## Proposing changes

For substantial changes, please open an issue to discuss the approach before sending a pull request.
Small fixes and improvements can be sent as a pull request directly.

When adding or changing supported syntax, include parser tests in `tests/parser.rs` and update the
supported-subset list in `README.md` and the crate-level docs in `src/lib.rs`.

## Code of Conduct

This project follows the [Contributor Covenant Code of Conduct](CODE_OF_CONDUCT.md). By
participating, you are expected to uphold this code. Report unacceptable behavior to
opensource@shopify.com.
