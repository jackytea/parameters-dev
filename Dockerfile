FROM rust:1.99.1-trixie AS build
ARG DX_VERSION=0.7.10
RUN name="dx-$(uname -m)-unknown-linux-gnu" \
    && base="https://github.com/DioxusLabs/dioxus/releases/download/v${DX_VERSION}" \
    && curl -sSfLo /tmp/dx.tar.gz "${base}/${name}.tar.gz" \
    && curl -sSfLo /tmp/dx.sha256 "${base}/${name}.sha256" \
    && grep -qi "$(sha256sum /tmp/dx.tar.gz | cut -d ' ' -f 1)" /tmp/dx.sha256 \
    && tar -xzf /tmp/dx.tar.gz -C /usr/local/cargo/bin dx \
    && rm /tmp/dx.tar.gz /tmp/dx.sha256
WORKDIR /src
ENV SQLX_OFFLINE=true
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    dx bundle --package parameters-app --platform web --release --out-dir /app

FROM debian:trixie-slim
RUN useradd --system --uid 10001 --user-group --create-home parameters \
    && install --directory --owner parameters --group parameters /workspace
COPY --from=build /app /app
USER parameters
WORKDIR /workspace
ENV PARAMETERS_ADDRESS=0.0.0.0:8080 \
    PARAMETERS_ALLOW_NON_LOOPBACK=true \
    PARAMETERS_WORKSPACE=/workspace
EXPOSE 8080
ENTRYPOINT ["/app/server"]
