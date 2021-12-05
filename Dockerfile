FROM rust:alpine as builder

RUN apk update
RUN apk add build-base

WORKDIR /usr/local/src/gemserv
COPY Cargo.lock Cargo.lock
COPY Cargo.toml Cargo.toml
COPY src src
COPY cgi-scripts cgi-scripts
RUN cargo build --release
RUN strip -s target/release/gemserv

## Second stage: single-binary container
FROM scratch

COPY --from=builder /usr/local/src/gemserv/target/release/gemserv /usr/local/bin/gemserv
ENTRYPOINT ["/usr/local/bin/gemserv"]
CMD ["/gemserv/config.toml"]
