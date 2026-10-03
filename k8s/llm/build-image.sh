#!/usr/bin/env bash
# Build llm-stack and optionally import into k3s containerd (no registry required).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
IMAGE="${IMAGE:-llm-stack:v0.9.0-rc.3-3}"

docker build --platform linux/amd64 -t "$IMAGE" "$ROOT/k8s/llm/build/llm-stack"

if [[ "${IMPORT:-0}" == "1" ]]; then
  docker save "$IMAGE" | sudo k3s ctr images import -
  echo "Imported $IMAGE into k3s"
else
  echo "Built $IMAGE. Import: IMPORT=1 $0"
  echo "  or: docker save $IMAGE | sudo k3s ctr images import -"
fi
