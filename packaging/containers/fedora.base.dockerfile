ARG FEDORA_VERSION=44
FROM fedora:${FEDORA_VERSION}

# Full OS update + all build-time deps in a single layer.
# This layer is cached as lam-builder-base-fedora; rebuild it with:
#   make container-refresh-fedora
RUN dnf update -y && \
    dnf install -y \
        rust \
        cargo \
        python3 \
        python3-devel \
        python3-pip \
        systemd-devel \
        rpm-build \
        rpmdevtools \
        libcap \
        git && \
    dnf clean all
