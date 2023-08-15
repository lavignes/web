#!/bin/sh

case "$(uname -sm)" in
    "Linux x86_64")
        cargo +nightly bloat -Z build-std=std,panic_abort -Z build-std-features=panic_immediate_abort --target x86_64-unknown-linux-gnu --release
        ;;
    *)
        cargo +nightly bloat --release
        ;;
esac

