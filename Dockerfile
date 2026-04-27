FROM rust:latest AS builder

RUN cargo install cargo-fuzz

ADD . /kiro-editor
WORKDIR /kiro-editor

RUN cd fuzz && cargo fuzz build

FROM ubuntu:20.04

COPY --from=builder /kiro-editor/fuzz/target/x86_64-unknown-linux-gnu/release/input_text /
