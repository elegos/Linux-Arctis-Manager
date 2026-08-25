FROM lam-builder-base-debian

# dpkg-buildpackage expects debian/ at the source root; ours lives under packaging/.
# We copy the source tree into a versioned directory as debuild requires.
WORKDIR /home/builder
COPY . source/

RUN set -e && \
    # Transform version: 3.0.0-alpha1 → 3.0.0~alpha1 (deb pre-release convention)
    VERSION=$(sed 's/-/~/g' source/VERSION) && \
    PKGNAME=linux-arctis-manager && \
    # Rename source dir to the debuild-expected name
    mv source ${PKGNAME}-${VERSION} && \
    # Symlink packaging/debian to the required location
    ln -s packaging/debian ${PKGNAME}-${VERSION}/debian && \
    cd ${PKGNAME}-${VERSION} && \
    # -us -uc: skip GPG signing (CI/container build)
    # -b: binary-only (no source package)
    dpkg-buildpackage -us -uc -b

# docker run copies artefacts to the host-mounted /out volume
CMD ["sh", "-c", \
    "cp -v /home/builder/*.deb /out/ && echo '.deb written to /out'"]
