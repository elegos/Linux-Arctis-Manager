ARG BASE_IMAGE=fedora:latest
FROM ${BASE_IMAGE}

# Only the tools to run rpmbuild and resolve BuildRequires; everything the
# build itself needs comes from the spec below.
RUN dnf install -y rpm-build rpmdevtools mold 'dnf-command(builddep)' && dnf clean all

# Build deps straight from the spec (as root), so this can't drift from it.
COPY packaging/fedora/linux-arctis-manager.spec /tmp/lam.spec
RUN dnf builddep -y /tmp/lam.spec && dnf clean all

# Unprivileged build user (rpmbuild never needs root). UID matches the host
# user so the bind-mounted /out stays writable.
ARG BUILDER_UID=1000
ARG BUILDER_GID=1000
RUN groupadd -o -g ${BUILDER_GID} builder && useradd -m -o -u ${BUILDER_UID} -g ${BUILDER_GID} builder

COPY --chown=builder:builder . /home/builder/source
USER builder
WORKDIR /home/builder/source

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
    "find /home/builder/rpmbuild/RPMS /home/builder/rpmbuild/SRPMS -name '*.rpm' \
         -exec cp -v {} /out/ \\; && echo 'RPMs written to /out'"]
