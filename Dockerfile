# linux dev environment: build, test, lint, and benchmark here, not on macOS.
# pinned so everyone (and every rerun of a benchmark) uses the same compiler
FROM rust:1.100-bookworm

RUN rustup component add rustfmt clippy

# build output lives in a docker volume, not in the bind-mounted repo:
# file I/O through the macOS bind mount is slow, and mac + linux builds don't mix in one target/
ENV CARGO_TARGET_DIR=/target

WORKDIR /workspace
