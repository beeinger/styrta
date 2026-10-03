# Qwen3.8-Flash-Next — measured on RTX 5060 Ti 16 GB

Ryzen 9 7900X, 128 GB DDR5-5600 (2 channels). UD-IQ4_XS + `mmproj-F16`. Unsloth `b10909-mix-bea84f7`. Decode = `timings.predicted_per_second`. `--cpu-moe` is DRAM-bandwidth bound on this topology.

**Live flags:** ctx 204800, q4 KV, `-b 2048 -ub 256`, GPU MTP shared-Q8 n-max 3.

| | tok/s | VRAM |
| --- | --- | --- |
| medium decode | **22.43** | 13044 MiB (llama idle) |
| + vision | — | 14792 MiB |
| + Whisper pinned | — | **14325 / 16311 MiB** |

| Date | Setup | Decode | Prefill ~4.8k | VRAM |
| --- | --- | --- | --- | --- |
| 2026-10-03 | live, ctx 204800 | **22.43** | — | 13044 |
| 2026-10-03 | same, ctx 122880 | 22.58 / 20.65 coding | 223 | 12130 |
| 2026-10-03 | MTP n3, 1024/512 | 21.75 | **321** | 12392 |
| 2026-10-03 | 1 MoE layer on GPU | 21.96 | 224 | 13666 |
| 2026-10-03 | 2 MoE layers on GPU | 21.74 | — | 15200 (too tight) |
| 2026-10-03 | ngram, no draft | 18.22 | 229 | 8738 |
| 2026-10-03 | ctx 262144 | 20.94 | — | 13682 |
| 2026-10-03 | MTP GPU n2, ctx 80k | **22.90** | 321 | 12286 |
| 2026-10-03 | MTP off | 18.24 | 330 | 9126 |
| 2026-10-03 | 80k, f16 KV | 16.33 | 629.6 | 13.63 GiB |

Keep extra VRAM for context, not extra MoE layers on 16 GB.
