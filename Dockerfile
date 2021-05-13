# syntax=docker/dockerfile:experimental
FROM rust:1.52-slim as builder
WORKDIR /work
COPY src src
COPY minion-cli minion-cli
COPY minion-ffi minion-ffi
COPY minion-tests minion-tests
COPY minion-codegen minion-codegen
COPY Cargo.toml Cargo.lock ./
RUN --mount=type=cache,target=/work/target,id=cargo,sharing=private  \
  cargo build -p minion-cli --release && cp ./target/release/minion-cli /mcli

FROM debian:stable-slim
COPY --from=builder /mcli /usr/bin/minion-cli
CMD ["/usr/bin/minion-cli"]
