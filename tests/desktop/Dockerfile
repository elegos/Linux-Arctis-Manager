# One image for every desktop; each test run starts a fresh container from it.
ARG BASE_IMAGE=fedora:45
FROM ${BASE_IMAGE}

# Headless X server, screenshot and input tools, then the libraries the
# PySide6 wheels load at runtime.
RUN dnf install -y \
        xorg-x11-server-Xvfb xdpyinfo xprop ImageMagick xdotool mesa-dri-drivers \
        dbus-daemon dbus-tools util-linux dejavu-sans-fonts glib2 \
        python3 python3-gobject python3-dbusmock \
        xorg-x11-server-Xwayland libcap procps-ng findutils \
        libxkbcommon-x11 xcb-util-cursor xcb-util-image xcb-util-keysyms \
        xcb-util-renderutil xcb-util-wm mesa-libEGL mesa-libGL fontconfig \
    && dnf clean all

# Cinnamon
RUN dnf install -y cinnamon xapps gcr3 mint-themes mint-y-icons && dnf clean all

# KDE Plasma
RUN dnf install -y \
        plasma-workspace kwin plasma-desktop \
        plasma-integration plasma-breeze breeze-icon-theme kf6-kconfig \
    && dnf clean all \
    # Containers cannot grant its scheduling capability, and with it set the
    # binary refuses to start.
    && setcap -r /usr/bin/kwin_wayland

# GNOME
RUN dnf install -y \
        gnome-shell gnome-shell-extension-appindicator gnome-session \
        adwaita-icon-theme \
    && dnf clean all

# Only the dependencies: the sources are mounted at run time, so code changes
# are tested without rebuilding the image.
COPY pyproject.toml /tmp/pyproject.toml
RUN python3 -m venv /opt/lam \
    && /opt/lam/bin/python -c "import tomllib; print('\n'.join(tomllib.load(open('/tmp/pyproject.toml', 'rb'))['project']['dependencies']))" > /tmp/requirements.txt \
    && /opt/lam/bin/pip install --no-cache-dir -r /tmp/requirements.txt
ENV PATH=/opt/lam/bin:$PATH

RUN useradd -m tester
