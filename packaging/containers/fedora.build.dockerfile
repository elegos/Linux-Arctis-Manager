FROM lam-builder-base-fedora

WORKDIR /build
COPY . .

RUN set -e && \
    # Transform version: 3.0.0-alpha1 → 3.0.0~alpha1 (RPM pre-release convention)
    VERSION=$(sed 's/-/~/g' VERSION) && \
    PKGNAME=linux-arctis-manager && \
    rpmdev-setuptree && \
    # Create source tarball from the working tree (excluding build artefacts)
    tar czf ~/rpmbuild/SOURCES/${PKGNAME}-${VERSION}.tar.gz \
        --transform "s,^\./,${PKGNAME}-${VERSION}/," \
        --exclude='./.git' \
        --exclude='./.venv' \
        --exclude='./dist' \
        --exclude='./daemon/target' \
        . && \
    # Patch version in spec (spec may still carry the alpha string from git)
    sed "s/^Version:.*/Version:        ${VERSION}/" \
        packaging/fedora/linux-arctis-manager.spec \
        > ~/rpmbuild/SPECS/${PKGNAME}.spec && \
    rpmbuild -ba ~/rpmbuild/SPECS/${PKGNAME}.spec

# docker run copies artefacts to the host-mounted /out volume
CMD ["sh", "-c", \
    "find /root/rpmbuild/RPMS /root/rpmbuild/SRPMS -name '*.rpm' \
         -exec cp -v {} /out/ \\; && echo 'RPMs written to /out'"]
