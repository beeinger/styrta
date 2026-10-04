# Styrta

Styrta is a conversation for people in Kraków and Małopolska. They speak or type. The transcript stays on screen. Most of the time that is the whole product: talk about a problem, a public service, or a social innovation, get it explained in plain language, and keep going.

The assistant answers from a library it has actually read, the [ROPS social-innovation catalog](https://rops.krakow.pl/innowacje-spoleczne/biblioteka-innowacji-spolecznych/kategorie). A person does not have to know that a project exists, or its name. They describe the situation. The reply can shorten a long pack of PDFs, say what the materials support, and name the page it came from. If the library has nothing, it says so.

The same thread remembers the person and can recommend. A recommendation is offered after it has listened, and only when it fits what they said. It might be an existing innovation, a public meetup, or someone with a shared interest. They can discuss it, refuse it, or leave. Nothing is posted on its own.

When they do want to go out, the meeting point is a public place in the region: a cafe, a park, a hall, a square. A home is rejected. Residents do not pay to be matched. The region is województwo małopolskie.

## What a person can do

Talk. Ask what something is, whether it works, who it is for, and what the test actually showed. The assistant stays on the subject, asks when it needs to, and can explain again in fewer words. A turn that only discusses does not write anything.

Ask what already exists. Search is by meaning. An older person who says they are bored and their memory is going can be shown BaWita, with the ROPS page, the licence, and the limit in the test report: the page claims an effect the report does not support, and the brief says so. A name still works. BaWita, Merkury, Uniodzież. The link in the reply is the page the search returned.

Talk about themselves. Age band, mobility, what they like and will not do, when they can go out, whether they will meet only women. Those facts are stored and can be corrected or forgotten. Later recommendations use them.

Ask where to go, once they want that. The assistant offers one meetup: what it is, the public place, and how many people are going, then asks if they want to go. Joining happens on a later turn, after a yes. If nothing fits, it asks whether they want to post their own, and waits.

Post a meetup. The assistant asks what it is, when, and which public place. It looks the place up (Nominatim, biased to Małopolska), shows the first hit on the map, and asks once whether that is the place — for example TAURON Arena Kraków at Stanisława Lema 7 in Kraków. The event is created only after that confirmation, at the coordinates the search returned.

See their own meetups, cancel, or mark one complete. Each of those is a separate confirmation.

Ask for company. The reply names the person as stored, the age band, shared interests, a distance in words, and which constraints passed. A quote, a mobility note, and a health fact stay off that card.

Switch language. Polish is the default. Asking for English is a profile update, and the next replies follow.

## How the assistant is kept honest

A discussion is speech. A lookup is one tool call, then speech that uses what came back: the short version, what the materials support, the page. The model does not write to the database. The app parses the call, rejects a bad one, and runs the tool. The spoken reply after a lookup has to come from the tool result.

| Tool | What it does |
| --- | --- |
| `remember`, `forget`, `set_profile` | Long-term and short-term memory, and the profile |
| `search_knowledge` | An innovation for the problem they described, with the page URL the crawl stored |
| `search_events`, `list_my_events` | Public meetups, and the ones this person is on |
| `search_people` | People who already passed the hard filters |
| `search_place` | A named public place, and a draft the map can draw |
| `create_event`, `join_event`, `cancel_attendance`, `complete_attendance` | Writes, only after the person asked |
| `search_chat_history` | Earlier turns in this conversation |

Hard filters run before any score. Cancelled, expired, and private places are dropped. An explicit dislike is dropped. When mobility says wheelchair, court sports and running are dropped before ranking: padel, tennis, basketball, volleyball, squash, badminton, football, and a run. Once the profile has any field, a women-only meetup is hidden unless the person asked for that. A full session can still be returned; the assistant is told not to offer it. Soft ranking (people already going, distance, text, time) never puts a dropped row back.

Search of the library mixes a vector score with title overlap. A name such as BaWita still works. Links on a stored brief come from the ROPS page. The digest is rejected if the model puts a URL in the text, names a file that was not in the pack, or leaves a required field empty. When the webpage claims more than the test report, the brief follows the report.

The assistant does not diagnose and does not treat.

## The library

ROPS already published the innovations, nine categories, each a page plus a zip of PDFs and documents. A resident, a gmina, or a ROPS worker still had to know the name.

The catalog is harvested from the nine category pages. One hundred and fifteen unique innovations are stored, with page URL, categories, licence, and the links on the page (including a film when the page has one). Text is taken from PDF, DOCX, ODT, and RTF inside the zip. An agent writes one Polish brief per project. Four hundred and sixty-two extracted files sit beside those briefs.

Three packs stay on the ROPS site because they are 11–28 GB: Dialog ponad kulturami, koMIX życiowy, Ścieżka motosensoryczna. Their briefs use the page and say the archive was not opened. Two pages had no archive: Ścieżka Feniksa and Inteligentny organizer do leków. Their briefs say the note is from the page.

Chunks are embedded with `intfloat/multilingual-e5-large` (1024 dimensions) and searched with pgvector. The national catalog at innowacjespoleczne.pl is not in this database.

## Repository

| Path | What it is |
| --- | --- |
| `backend/` | Rust service. HTTP API (`serve`), library ingest (`styrta ingest`), Postgres |
| `frontend/` | Expo map of public activity around TAURON Arena. The chat sheet on that screen is local; it does not call the agent yet |
| `k8s/llm/` | Pre-existing GPU pod: chat, speech recognition, Polish speech. See [`k8s/README.md`](k8s/README.md) |
| `k8s/embeddings/` | CPU embeddings, `multilingual-e5-large` |
| `k8s/postgres/` | Postgres with pgvector |
| `k8s/api/` | The `serve` deployment, ingest and embed jobs |

`knowledge/` is a separate notes repo and is gitignored here. It is not required to build or run the app.

The GPU pod, the weights, llama.cpp, Whisper, and Piper were already running before this application. Resident conversation is designed to stay on that machine: chat, speech in, and speech out. There is no outbound LLM in that path. The briefs were written by a separate digest pass; they are stored text, not a live call during a resident’s turn.

## Models

One GPU pod on k3s, runtime class `nvidia`. llama.cpp serves Qwen3.8-Flash-Next (Unsloth UD-IQ4_XS, the name used in this repo) on an OpenAI-compatible API. Experts run on CPU (`--cpu-moe`) and sit in RAM, about 94 GB locked. Attention and the KV cache stay on the GPU. Measured decode on an RTX 5060 Ti 16 GB with 128 GB of system RAM is 22.43 tok/s. Context is 204800. The model does not fit in 16 GB of VRAM.

Speech recognition is faster-whisper large-v3-turbo. Speech out is Piper `pl_PL-darkman-medium` on CPU. Embeddings are a separate CPU service and do not use the GPU.

Numbers, flags, and the bench notes are in [`k8s/README.md`](k8s/README.md) and [`k8s/llm/BENCHMARKS.md`](k8s/llm/BENCHMARKS.md).

## API

`serve` listens on `:8088`. OpenAPI is at `/docs`.

Accounts are email and password. The password is hashed with Argon2id. Register and login return a JWT access token (one hour) and a refresh token (thirty days). Every other route needs `Authorization: Bearer <access_token>`.

| | |
| --- | --- |
| `POST /v1/auth/register` | Email, password, display name |
| `POST /v1/auth/login` | Email and password |
| `POST /v1/auth/refresh` | New access token |
| `GET /v1/stream` | Server-sent events for this user, including a place draft |
| `GET`, `PATCH /v1/me` | Profile |
| `GET /v1/events/nearby` | Public meetups in view |
| `POST /v1/events` and `…/join`, `…/cancel`, `…/complete` | The same writes the assistant uses |
| `GET /v1/me/events` | Meetups this person is on |
| `POST /v1/chat/messages` | A turn. Text, or audio that is transcribed first |
| `GET /v1/chat/turns/{id}` | The turn, including tool calls |
| `GET /v1/chat/turns/{id}/audio` | Speech for that reply |

The phone map still reads a starter set of pins around TAURON Arena (ul. Stanisława Lema 7). A session that is already full is not drawn. Host and headcount are what a pin shows. Those pins are not a live city feed.

## Run

Postgres needs `vector`, `pg_trgm`, and `unaccent`. From `backend/`:

```bash
cargo test
cargo run --bin serve
cargo run --bin styrta -- ingest status
```

`serve` applies migrations on startup. Ingest and the embed fill are separate commands and cluster jobs.

Required for `serve`:

| Variable | |
| --- | --- |
| `DATABASE_URL` | Postgres |
| `STYRTA_JWT_SECRET` | Signing key for access and refresh tokens |
| `LLM_BASE_URL`, `LLM_API_KEY`, `LLM_MODEL` | Chat. `LLM_TIMEOUT_SECS` defaults to 120 |
| `SPEECH_BASE_URL`, `STT_MODEL`, `TTS_MODEL`, `TTS_VOICE` | Transcription and speech |
| `EMBED_BASE_URL`, `EMBED_MODEL`, `EMBED_QUERY_PREFIX`, `EMBED_PASSAGE_PREFIX` | 1024-d embeddings. The query prefix may be empty; the variable must be set |

Optional: `SPEECH_API_KEY`, `EMBED_API_KEY`, `LLM_USER_AGENT`, `STYRTA_AUDIO_DIR` (default `/var/lib/styrta/audio`), `GEOCODER_BASE_URL` (default Nominatim), `GEOCODER_USER_AGENT`.

Ingest (`styrta ingest`) needs `DATABASE_URL` and the chat variables. `serve embed` needs `DATABASE_URL` and the embed variables.

The cluster layout is `k8s/`. The public API host in the ingress is `api.styrta.energia.dev`.

## What this build does not include

The idea form, the grant generator, tester ratings, a mentor inbox, an admin editor, and a middleman that rewrites one innovation into a service for a gmina. The national innovation portal. A live feed of city events. Wiring the phone’s chat sheet to this API.

Medical diagnosis, private addresses, and a launch outside Małopolska are out of scope.
