FROM aegooby/rust-fuzz:latest AS builder

ADD . /kiro-editor
WORKDIR /kiro-editor

RUN cd fuzz && ${HOME}/.cargo/bin/cargo fuzz build

FROM ubuntu:20.04

COPY --from=builder /kiro-editor/fuzz/target/x86_64-unknown-linux-gnu/release/input_text /
