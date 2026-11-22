# macOS dispatch dependency backport

Application source base: b0b9cc55ccf7c34094b735453952c3e7a8a96a5e. This
macOS candidate changes no application source. The vendored Foundation source
was extracted from the upstream Cargo lockfile checksum for objc2-foundation
0.2.2 (VCS e282618be4c3a3b9542957e0c8540e9588472ce8).

The optional dispatch feature points to dispatch2 0.3.1 (pinned upstream
VCS 8852b424193ca41602281b3d7540d7c8ed51e49a) and keeps its old API and
main-thread behavior at its sole dispatch call site. It synchronously
dispatches to Apple's main queue and captures/resumes any Rust panic on the
caller without crossing the C callback boundary. The old dispatch 0.2.0
crate is absent from the aarch64-apple-darwin normal dependency graph.
Private proof: 53 application tests passed and a synthetic native main-loop
harness verified return values, main-thread markers, and panic propagation.
