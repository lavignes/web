# `web` Browser

`web` is a moderately serious no-script browser written in Rust.

## Goals

* Implement the full\* [HTML spec](https://html.spec.whatwg.org/multipage/)
* Fully async implementation\*\* (i.e. no thread per-tab, all work shared in common pool)
* Complete binary smaller than 1 MiB
* Terminal and Graphical UI
* Minimal dependencies\*\*\*

\* See Non-Goals

\*\* Including async parsing of supported media-types

\*\*\* A basic non-TLS terminal build should only require an async runtime

## Non-Goals

* Javascript

