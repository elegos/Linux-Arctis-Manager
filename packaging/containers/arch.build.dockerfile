FROM lam-builder-base-arch

# Copy source tree as root, then hand ownership to builder
COPY --chown=builder:builder . /home/builder/source

# Copy the local-source PKGBUILD and the post-install script
COPY --chown=builder:builder packaging/containers/arch.local.PKGBUILD \
                              /home/builder/pkgbuild/PKGBUILD
COPY --chown=builder:builder packaging/arch/lam.install \
                              /home/builder/pkgbuild/lam.install

USER builder
WORKDIR /home/builder/pkgbuild

RUN set -e && \
    # Arch pkgver must not contain '-'; strip it (3.0.0-alpha1 → 3.0.0alpha1)
    VERSION=$(sed 's/-//g' /home/builder/source/VERSION) && \
    sed -i "s/^pkgver=.*/pkgver=${VERSION}/" PKGBUILD && \
    # -s: install missing deps via sudo pacman
    # --skippgpcheck: no GPG key needed for local source
    # --noconfirm: non-interactive
    makepkg -s --noconfirm --skippgpcheck

# docker run copies artefacts to the host-mounted /out volume
CMD ["sh", "-c", \
    "cp -v /home/builder/pkgbuild/*.pkg.tar.zst /out/ && echo '.pkg.tar.zst written to /out'"]
