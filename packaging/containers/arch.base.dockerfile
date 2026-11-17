FROM archlinux:latest

# Full system update + all build-time deps in a single layer.
# This layer is cached as lam-builder-base-arch; rebuild it with:
#   make container-refresh-arch
RUN pacman -Syu --noconfirm && \
    pacman -S --noconfirm --needed \
        rust \
        cargo \
        python \
        base-devel \
        git && \
    pacman -Sc --noconfirm
