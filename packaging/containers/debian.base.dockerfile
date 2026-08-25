ARG DEBIAN_VERSION=24.04
FROM ubuntu:${DEBIAN_VERSION}

ENV DEBIAN_FRONTEND=noninteractive

# Full OS update + all build-time deps in a single layer.
# This layer is cached as lam-builder-base-debian; rebuild it with:
#   make container-refresh-debian
RUN apt-get update && \
    apt-get upgrade -y && \
    apt-get install -y --no-install-recommends \
        cargo \
        rustc \
        python3 \
        python3-venv \
        python3-pip \
        debhelper \
        devscripts \
        libcap2-bin \
        git && \
    # uv is not in Ubuntu repos; install via pip into the system Python
    pip3 install uv --break-system-packages && \
    apt-get clean && \
    rm -rf /var/lib/apt/lists/*
