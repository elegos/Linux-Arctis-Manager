ARG DEBIAN_VERSION=24.04
FROM ubuntu:${DEBIAN_VERSION}

ENV DEBIAN_FRONTEND=noninteractive

# Full OS update + all build-time deps in a single layer.
# This layer is cached as lam-builder-base-debian; rebuild it with:
#   make container-refresh-debian
RUN apt-get update && \
    apt-get upgrade -y && \
    apt-get install -y --no-install-recommends \
        curl \
        pkg-config \
        libudev-dev \
        libssl-dev \
        python3 \
        python3-venv \
        python3-pip \
        debhelper \
        devscripts \
        libcap2-bin \
        git && \
    apt-get clean && \
    rm -rf /var/lib/apt/lists/*
# cargo/rustc deliberately not from apt: Debian/Ubuntu's packaged rustc is
# routinely behind this project's MSRV. debian/rules installs a current
# toolchain via rustup itself if `cargo` isn't already found on PATH.
