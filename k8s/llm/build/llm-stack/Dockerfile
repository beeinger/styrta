# llm-stack: ONE container running llama-server (GPU, CUDA 13) + speaches
# (whisper STT on GPU via ctranslate2 CUDA 12 pip libs; Piper TTS on CPU via
# onnxruntime CPU EP) under s6-overlay supervision.
#
# Base is the CUDA 13 runtime image the llama.cpp Unsloth binary (b10909-mix,
# cuda13 build) is proven against — llama-server links libcudart/libcublas 13
# from /usr/local/cuda. speaches' GPU stack (ctranslate2) gets its CUDA 12
# runtime + cuBLAS + cuDNN 9 from pip nvidia wheels inside the venv,
# registered with ldconfig (same trick as the upstream speaches Dockerfile).
# The two CUDA major versions coexist: different sonames (.12 / .13).
#
# Build: k8s/llm/build-image.sh   (tag llm-stack:v0.9.0-rc.3-3)

ARG BASE_IMAGE=nvidia/cuda:13.0.1-runtime-ubuntu24.04@sha256:c3fde347d52d578c84fd644bc177bc7ec333feaf11550d990da4084d7612e4c7

FROM ${BASE_IMAGE}

ARG SPEACHES_VERSION=v0.9.0-rc.3
ARG S6_VERSION=v3.2.3.2
# sha256 of s6-overlay-noarch.tar.xz and s6-overlay-x86_64.tar.xz (release assets)
ARG S6_NOARCH_SHA256=5379750ed30a84bbd2e2dd74847ba6b5bd29cd0b2e3ea2ec58049b57eb2eda12
ARG S6_X86_64_SHA256=e6befcc96a437a3831386ecfc51808c5d3e939dc5fe3c02ae9284599e8aa2408

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates \
      curl \
      ffmpeg \
      git \
      libgomp1 \
      python3 \
      python3-venv \
      xz-utils \
    && rm -rf /var/lib/apt/lists/*

# --- speaches (pinned release, built from source into its own venv) ---
# uv version must satisfy speaches' pyproject `required-version = "~=0.8.14"`.
COPY --from=ghcr.io/astral-sh/uv:0.8.22@sha256:9874eb7afe5ca16c363fe80b294fe700e460df29a55532bbfea234a0f12eddb1 /uv /usr/local/bin/uv

WORKDIR /opt/speaches
RUN git clone --depth 1 --branch "${SPEACHES_VERSION}" \
      https://github.com/speaches-ai/speaches.git /opt/speaches \
    && uv sync --frozen --compile-bytecode --no-dev \
    # CUDA 12 runtime libs for ctranslate2 (whisper on GPU): cuBLAS, cuDNN 9,
    # cudart — the CUDA 13 base image ships none of the .12 sonames.
    && uv pip install --python .venv/bin/python \
         nvidia-cublas-cu12 nvidia-cudnn-cu12 nvidia-cuda-runtime-cu12

USER root
# Register the venv's pip-installed NVIDIA lib dirs with the dynamic linker
# (upstream speaches Dockerfile does the same for its CUDA 12 base).
RUN find /opt/speaches/.venv -maxdepth 7 -path "*/nvidia/*/lib" -type d \
      > /etc/ld.so.conf.d/venv-nvidia.conf \
    && ldconfig

# --- model alias override (speaches resolves model_aliases.json from CWD) ---
# tts-1 must NOT map to Kokoro (no Polish voice); map it to Piper darkman.
# whisper-1 maps to faster-whisper large-v3-turbo (multilingual, incl. Polish).
COPY model_aliases.json /opt/speaches/model_aliases.json

# --- s6-overlay (process supervision for the two services) ---
RUN set -eux; \
    curl -fsSL --retry 3 -o /tmp/s6-noarch.tar.xz \
      "https://github.com/just-containers/s6-overlay/releases/download/${S6_VERSION}/s6-overlay-noarch.tar.xz"; \
    curl -fsSL --retry 3 -o /tmp/s6-x86_64.tar.xz \
      "https://github.com/just-containers/s6-overlay/releases/download/${S6_VERSION}/s6-overlay-x86_64.tar.xz"; \
    echo "${S6_NOARCH_SHA256}  /tmp/s6-noarch.tar.xz" | sha256sum -c -; \
    echo "${S6_X86_64_SHA256}  /tmp/s6-x86_64.tar.xz" | sha256sum -c -; \
    tar -C / -Jxpf /tmp/s6-noarch.tar.xz; \
    tar -C / -Jxpf /tmp/s6-x86_64.tar.xz; \
    rm -f /tmp/s6-*.tar.xz

COPY s6-rc.d /etc/s6-overlay/s6-rc.d/
# Services and oneshot scripts are exec'd directly by s6-rc: force the
# executable bit (git/tar transfer modes are irrelevant).
RUN find /etc/s6-overlay/s6-rc.d -type f \( -name run -o -name up -o -name script -o -name health-check \) \
      -exec chmod +x {} +

ENV UVICORN_HOST=0.0.0.0 \
    UVICORN_PORT=8000 \
    PATH=/opt/speaches/.venv/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    HF_HUB_ENABLE_HF_TRANSFER=0 \
    DO_NOT_TRACK=1 \
    GRADIO_ANALYTICS_ENABLED=False \
    DISABLE_TELEMETRY=1 \
    HF_HUB_DISABLE_TELEMETRY=1

EXPOSE 8000 8080

ENTRYPOINT ["/init"]
