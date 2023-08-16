#!/bin/sh

case "$(uname -sm)" in
    "Linux x86_64")
        cargo clean
        cargo +nightly bloat -Z build-std=std,panic_abort -Z build-std-features=panic_immediate_abort --target x86_64-unknown-linux-gnu --release
        strip -s target/x86_64-unknown-linux-gnu/release/web
        ls -lh target/x86_64-unknown-linux-gnu/release/web
        ;;
    *)
        cargo clean
        cargo +nightly bloat --release
        strip -s target/release/web
        ls -l target/release/web
        ;;
esac

