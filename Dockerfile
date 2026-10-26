FROM rust:1.98-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked && cp target/release/catsed /catsed

FROM scratch
COPY --from=build /catsed /catsed
USER 65534:65534
EXPOSE 8080 9090
ENTRYPOINT ["/catsed"]
