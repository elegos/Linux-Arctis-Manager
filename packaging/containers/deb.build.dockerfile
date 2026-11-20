# Builds the .deb on a real Debian/Ubuntu/... image, mirroring the CI job in
# .github/workflows/build-pkg.yaml: build deps come straight from
# packaging/debian/control (mk-build-deps), nothing is hand-listed here.
#
#   make container-build-debian-deb BASE_IMAGE=debian:stable-slim
ARG BASE_IMAGE=debian:stable-slim
FROM ${BASE_IMAGE}

ARG DEB_SLUG=debian

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        devscripts debhelper equivs dpkg-dev fakeroot ca-certificates && \
    apt-get clean

# Unprivileged build user. UID matches the host user so the bind-mounted
# /out stays writable; some images (ubuntu) already ship a user at UID 1000.
ARG BUILDER_UID=1000
ARG BUILDER_GID=1000
RUN (id -nu ${BUILDER_UID} 2>/dev/null | xargs -r userdel -r) ; \
    groupadd -o -g ${BUILDER_GID} builder && \
    useradd -m -o -u ${BUILDER_UID} -g ${BUILDER_GID} builder

WORKDIR /tmp/build
COPY . source/

# Root only for what needs it: installing the build deps.
RUN set -e && \
    VERSION=$(sed 's/-/~/g' source/VERSION) && \
    PKGNAME=linux-arctis-manager && \
    mv source ${PKGNAME}-${VERSION} && \
    cd ${PKGNAME}-${VERSION} && \
    ln -s packaging/debian debian && \
    # Tag the version with the base image so .debs built for different
    # distros don't share a filename (same trick as the CI workflow).
    sed -i "0,/^linux-arctis-manager (.*) /s//linux-arctis-manager (${VERSION}+${DEB_SLUG}-1) /" debian/changelog && \
    apt-get update && \
    mk-build-deps --tool 'apt-get -y --no-install-recommends' debian/control && \
    apt-get -y --no-install-recommends install ./linux-arctis-manager-build-deps_*.deb && \
    apt-get clean && \
    chown -R builder:builder /tmp/build

USER builder
RUN cd /tmp/build/linux-arctis-manager-* && dpkg-buildpackage -us -uc -b

CMD ["sh", "-c", "cp -v /tmp/build/*.deb /out/ && echo '.deb written to /out'"]
