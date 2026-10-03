# LLM + STT + TTS (k3s)

One GPU pod: **llama.cpp** (Qwen3.8-Flash-Next UD-IQ4_XS) and **speaches** (Whisper STT + Piper TTS).

## Host (GPU node)

1. NVIDIA driver (`nvidia-smi` works) and `nvidia-container-toolkit`.
2. Register the runtime with k3s containerd, then restart k3s:

```bash
sudo k8s/nvidia-device-plugin/host-setup.sh
sudo systemctl restart k3s
```

3. Label that node and keep the GPU exclusive to this stack:

```bash
kubectl label node <gpu-node> accelerator=nvidia --overwrite
```

4. Default StorageClass `local-path` (k3s) with ≥125 GiB free on that node.
5. Node RAM: llama mlocks ~94 GiB; requests **12 CPU / 96 GiB**, limits **22 CPU / 110 GiB**.

## Image

```bash
k8s/llm/build-image.sh
IMPORT=1 k8s/llm/build-image.sh   # docker save | k3s ctr images import
```

Image name in the Deployment: `llm-stack:v0.9.0-rc.3-3` (`imagePullPolicy: IfNotPresent`). Point it at a registry if you have one.

## Apply

```bash
kubectl apply -k k8s/nvidia-device-plugin/

cp k8s/llm/secret-api-key.example.yaml k8s/llm/secret-api-key.yaml
# openssl rand -hex 32 → api-key
kubectl apply -f k8s/llm/secret-api-key.yaml

kubectl apply -k k8s/llm/
```

First boot: init containers pull GGUF (~94 GB) + Unsloth `llama-server` onto the model PVC (cached after). `startupProbe` waits up to 60 min for mlock. Speaches fills `llama-hf-cache`.

```bash
kubectl get pods -n nvidia-device-plugin
kubectl describe node <gpu-node> | grep -A3 nvidia.com/gpu
kubectl get pods -n llm -w
```

Plugin log `libnvidia-ml.so.1` not found: keep the File hostPath mounts in the DaemonSet (do not put host libc on `LD_LIBRARY_PATH`). Then `kubectl delete pod -n nvidia-device-plugin -l name=nvidia-device-plugin-ds`.

## Stack

| | |
| --- | --- |
| GPU | 16 GB class (benched on RTX 5060 Ti). RuntimeClass `nvidia`. |
| Weights | Unsloth UD-IQ4_XS (~93.7 GB) + `mmproj-F16.gguf` + MTP shared-Q8 |
| Binary | Unsloth `b10909-mix-bea84f7` CUDA 13 → PVC `/models/bin` |
| llama flags | `k8s/llm/build/llm-stack/s6-rc.d/llama-server/run` — ctx 204800, `--cpu-moe`, q4 KV, GPU MTP n3 |
| STT | faster-whisper large-v3-turbo int8 on GPU, pinned |
| TTS | Piper `pl_PL-darkman-medium` on CPU |

Benches: [`llm/BENCHMARKS.md`](llm/BENCHMARKS.md). Combined idle VRAM **14325 / 16311 MiB**. If headroom under load is under ~0.5 GiB, drop Whisper to medium int8 (`PRELOAD_MODELS` + `model_aliases.json`).

## APIs

| Service | In-cluster | |
| --- | --- | --- |
| `llama-cpp.llm` | `:80` → `8080` | OpenAI chat. Bearer = `llm-api-credentials`. `/health` public. `/metrics` same port, keyed. |
| `llama-cpp-speaches.llm` | `:80` → `8000` | `POST /v1/audio/transcriptions`, `POST /v1/audio/speech`, `GET /health` |

Aliases: `whisper-1` → turbo CT2; `tts-1` / `tts-1-hd` → Piper darkman.

```bash
kubectl -n llm port-forward svc/llama-cpp 8080:80
curl -sS http://127.0.0.1:8080/health
curl -sS http://127.0.0.1:8080/v1/chat/completions \
  -H "Authorization: Bearer $LLAMA_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"messages":[{"role":"user","content":"Reply with exactly: pong"}],"max_tokens":32}'

kubectl -n llm port-forward svc/llama-cpp-speaches 8000:80
curl -sS http://127.0.0.1:8000/health
```

Long `/health` 503 on first start is weight load + mlock.

| Symptom | Check |
| --- | --- |
| Pending, GPU | node unlabeled, plugin down, or another pod holds `nvidia.com/gpu` |
| Pending, memory | allocatable vs 96 GiB request |
| ImagePullBackOff | `IMPORT=1 k8s/llm/build-image.sh` |
| CUDA OOM | lower batch/ubatch in `llama-server/run`, or smaller Whisper |
| `mlock` denied | `IPC_LOCK` on the container |
| audio 404 | client base URL must include `/v1` |
| whisper re-download | `llama-hf-cache` mounted at `/hf-cache` |
