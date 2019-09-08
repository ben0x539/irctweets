let
  pkgs' = import <nixpkgs> {};
  nixpkgs-mozilla = pkgs'.fetchFromGitHub {
    owner = "mozilla";
    repo = "nixpkgs-mozilla";
    rev = "e37160aaf4de5c4968378e7ce6fe5212f4be239f";
    sha256 = "013hapfp76s87wiwyc02mzq1mbva2akqxyh37p27ngqiz0kq5f2n";
  };
  rust-overlay = import "${nixpkgs-mozilla}/rust-overlay.nix";
  pkgs = import <nixpkgs> { overlays = [ rust-overlay ]; };
  nightly-rust = (pkgs.rustChannelOf {
    date = "2019-09-07";
    channel = "nightly";
  }).rust;
in

with pkgs;

mkShell {
  buildInputs = [
    sqlite
    openssl
  ];
  nativeBuildInputs = [
    nightly-rust
    pkgconfig
  ];
}
