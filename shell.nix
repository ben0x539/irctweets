with (import ./nix/nixpkgs.nix);

mkShell {
  buildInputs = [
    sqlite
    openssl
  ];
  nativeBuildInputs = [
    rust
    pkgconfig
  ];
}
