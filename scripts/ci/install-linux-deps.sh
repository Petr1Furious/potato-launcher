#!/usr/bin/env bash
set -euo pipefail

sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  pkg-config \
  libssl-dev \
  libfontconfig-dev \
  libwayland-dev \
  libxkbcommon-dev \
  libxkbcommon-x11-dev \
  libxcb1-dev \
  libxcb-render0-dev \
  libxcb-shape0-dev \
  libxcb-xfixes0-dev \
  libx11-dev \
  libxext-dev \
  libxcursor-dev \
  libxrandr-dev \
  libxi-dev \
  libgl1-mesa-dev \
  libvulkan-dev \
  libasound2-dev
