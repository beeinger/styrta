#!/usr/bin/env bash
# GPU node: wire nvidia-container-runtime into k3s containerd (RuntimeClass handler "nvidia").
# Driver + nvidia-container-toolkit must already be installed. nvidia-smi must work.
set -euo pipefail

CONFIG="${NVIDIA_CONTAINERD_CONFIG:-/var/lib/rancher/k3s/agent/etc/containerd/config.toml}"

if ! command -v nvidia-smi >/dev/null; then
  echo "nvidia-smi not found; install the NVIDIA driver first" >&2
  exit 1
fi
nvidia-smi >/dev/null

if ! command -v nvidia-ctk >/dev/null; then
  echo "nvidia-ctk not found; install nvidia-container-toolkit first" >&2
  exit 1
fi

if [[ ! -f "$CONFIG" ]]; then
  echo "k3s containerd config missing: $CONFIG (is k3s installed?)" >&2
  exit 1
fi

nvidia-ctk runtime configure --runtime=containerd --config "$CONFIG"
echo "Configured NVIDIA runtime in $CONFIG"
echo "Restart k3s, then: kubectl label node <gpu-node> accelerator=nvidia --overwrite"
echo "  sudo systemctl restart k3s"
