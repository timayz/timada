# syntax=docker/dockerfile:1
# The image the release build starts from (`Dockerfile`): the toolchain, the
# two CLIs the build runs, and a first build of the whole dashboard, so a
# release only recompiles what changed since. Rebuilt by hand (the Builder
# workflow) when the toolchain or many dependencies move.
FROM rust:1.99-alpine

# build-base, cmake and perl build the C the crypto and WebP crates carry;
# clang-libs is what the store's bindings generator looks for; libstdc++ and
# libgcc are what the Tailwind CLI runs on.
RUN apk add --no-cache musl-dev build-base cmake perl clang-libs \
        openssl-dev openssl-libs-static \
        pkgconf git curl ca-certificates \
        libstdc++ libgcc

ARG USER=timada
ARG UID=10001
ARG GID=10001

# Create group explicitly with fixed GID, then user with fixed UID/GID.
# This guarantees GID=10001 (adduser alone may pick a different GID).
RUN addgroup -g "${GID}" -S "${USER}" && \
    adduser \
        --disabled-password \
        --gecos "" \
        --home "/nonexistent" \
        --shell "/sbin/nologin" \
        --no-create-home \
        --uid "${UID}" \
        --ingroup "${USER}" \
        "${USER}"

# Set `SYSROOT` to a dummy path (default is /usr) because pkg-config-rs *always*
# links those located in that path dynamically but we want static linking, c.f.
# https://github.com/rust-lang/pkg-config-rs/blob/54325785816695df031cef3b26b6a9a203bbc01b/src/lib.rs#L613
ENV SYSROOT=/dummy

# The dashboard's build.rs runs Tailwind. The release topcoat would download
# is linked against glibc and does not start on Alpine, so the musl build of
# the same version is fetched here and named to it. Keep the version in step
# with topcoat's (`DEFAULT_VERSION` in topcoat-tailwind) and the checksums
# with the release's (`gh api repos/tailwindlabs/tailwindcss/releases/tags/v<version>`).
ARG TAILWIND_VERSION=4.3.2
ARG TAILWIND_SHA256_AMD64=ae828e9e989ecbddb2bef856af8b0308ba162583b4922b3a065b5e26f86b0691
ARG TAILWIND_SHA256_ARM64=24a0dd39cbbced9d94f6313a747cc29ab2523a6a7b69204f2151e0af6aad6eef
ARG TARGETARCH
RUN case "${TARGETARCH}" in \
        amd64) asset=tailwindcss-linux-x64-musl; sha="${TAILWIND_SHA256_AMD64}" ;; \
        arm64) asset=tailwindcss-linux-arm64-musl; sha="${TAILWIND_SHA256_ARM64}" ;; \
        *) echo "no Tailwind CLI for ${TARGETARCH}" >&2; exit 1 ;; \
    esac && \
    curl -fsSL -o /usr/local/bin/tailwindcss \
        "https://github.com/tailwindlabs/tailwindcss/releases/download/v${TAILWIND_VERSION}/${asset}" && \
    echo "${sha}  /usr/local/bin/tailwindcss" | sha256sum -c - && \
    chmod +x /usr/local/bin/tailwindcss
ENV TAILWIND_CLI=/usr/local/bin/tailwindcss

# `topcoat asset bundle` builds the binary and writes the files it serves
# next to it. Pinned to the topcoat version the workspace uses: another one
# writes a bundle the binary refuses.
ARG TOPCOAT_CLI_VERSION=0.12.1
RUN cargo install topcoat-cli --version "${TOPCOAT_CLI_VERSION}" --locked

WORKDIR /app

COPY . .

RUN topcoat asset bundle -p timada-dashboard --release
