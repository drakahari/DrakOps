# macOS dispatch dependency backport

This is the dispatch backport carried forward unchanged from the public
Apple Silicon RC2 source (fc035e8bc40e7cbee2efc6c8ec87ec6f4587a014). The vendored Foundation source
was extracted from the upstream Cargo lockfile checksum for objc2-foundation
0.2.2 (VCS e282618be4c3a3b9542957e0c8540e9588472ce8).

The optional dispatch feature points to dispatch2 0.3.1 (pinned upstream
VCS 8852b424193ca41602281b3d7540d7c8ed51e49a) and keeps its old API and
main-thread behavior at its sole dispatch call site. It synchronously
dispatches to Apple's main queue and captures/resumes any Rust panic on the
caller without crossing the C callback boundary. The old dispatch 0.2.0
crate is absent from the aarch64-apple-darwin normal dependency graph.
Prior RC2 verification: 53 application tests passed and a synthetic native main-loop
harness verified return values, main-thread markers, and panic propagation.
