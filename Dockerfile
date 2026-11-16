FROM rust:1.99.1-trixie AS build
WORKDIR /src
ENV SQLX_OFFLINE=true
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --package parameters-app --features server \
    && cp target/release/parameters-app /usr/local/bin/parameters-server

FROM debian:trixie-slim
RUN useradd --system --uid 10001 --user-group --create-home parameters \
    && install --directory --owner parameters --group parameters /workspace
COPY --from=build /usr/local/bin/parameters-server /usr/local/bin/parameters-server
USER parameters
WORKDIR /workspace
ENV PARAMETERS_ADDRESS=0.0.0.0:8080 \
    PARAMETERS_ALLOW_NON_LOOPBACK=true \
    PARAMETERS_WORKSPACE=/workspace
EXPOSE 8080
ENTRYPOINT ["parameters-server"]
