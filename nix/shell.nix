{
  mkShell,
  cargo,
  lld,
  rust-analyzer,
  rustc,
  rustfmt,
  clippy,
  cargo-audit,
  cargo-nextest,
}:
mkShell {
  name = "watchdog";
  strictDeps = true;
  nativeBuildInputs = [
    cargo
    rustc
    lld

    clippy
    rust-analyzer
    (rustfmt.override {asNightly = true;})

    # Additional Cargo Tooling
    cargo-audit
    cargo-nextest
  ];
}
