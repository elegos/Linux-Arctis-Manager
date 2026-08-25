FROM archlinux:latest

# Full system update + all build-time deps in a single layer.
# This layer is cached as lam-builder-base-arch; rebuild it with:
#   make container-refresh-arch
RUN pacman -Syu --noconfirm && \
    pacman -S --noconfirm --needed \
        rust \
        cargo \
        python \
        uv \
        base-devel \
        git && \
    pacman -Sc --noconfirm

# makepkg refuses to run as root; create a dedicated build user
RUN useradd -m builder && \
    echo 'builder ALL=(ALL) NOPASSWD: ALL' >> /etc/sudoers
