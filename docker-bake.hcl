# Every Granary image in one build: `docker buildx bake -f docker-bake.hcl` (all) or `... -f docker-bake.hcl api pdf`.
# (Name the file: without -f, bake also merges the services of docker-compose.yml.) The Rust targets share one
# compile stage, so the workspace is built once for all of them. CI (.github/workflows/images.yml) runs this on
# each release tag and pushes to ghcr.io/halogass/granary-*.

variable "REGISTRY" {
  default = "ghcr.io/halogass"
}

variable "VERSION" {
  default = "dev"
}

# Extra tags besides VERSION, comma separated (CI sets "1.2,latest" for 1.2.3).
variable "EXTRA_TAGS" {
  default = ""
}

variable "CARGO_BUILD_JOBS" {
  default = "2"
}

function "tags" {
  params = [name]
  result = concat(
    ["${REGISTRY}/granary-${name}:${VERSION}"],
    [for t in compact(split(",", EXTRA_TAGS)) : "${REGISTRY}/granary-${name}:${t}"]
  )
}

function "labels" {
  params = [title, description]
  result = {
    "org.opencontainers.image.title"       = title
    "org.opencontainers.image.description" = description
    "org.opencontainers.image.source"      = "https://github.com/halogass/Granary-ERP"
    "org.opencontainers.image.licenses"    = "MIT OR Apache-2.0"
    "org.opencontainers.image.version"     = VERSION
  }
}

group "default" {
  targets = ["api", "pdf", "face", "signal", "research", "frontend", "stt"]
}

target "_rust" {
  context    = "rust-version"
  dockerfile = "Dockerfile"
  args       = { CARGO_BUILD_JOBS = CARGO_BUILD_JOBS }
  platforms  = ["linux/amd64"]
}

target "api" {
  inherits = ["_rust"]
  target   = "api"
  tags     = tags("api")
  labels   = labels("granary-api", "Granary backend, worker and beat (one image, three commands)")
}

target "pdf" {
  inherits = ["_rust"]
  target   = "pdf"
  tags     = tags("pdf")
  labels   = labels("granary-pdf", "Granary PDF renderer (Chromium)")
}

target "face" {
  inherits = ["_rust"]
  target   = "face"
  tags     = tags("face")
  labels   = labels("granary-face", "Granary face matching service (ONNX Runtime)")
}

target "signal" {
  inherits = ["_rust"]
  target   = "signal"
  tags     = tags("signal")
  labels   = labels("granary-signal", "Granary WebRTC signalling for calls")
}

target "research" {
  inherits = ["_rust"]
  target   = "research"
  tags     = tags("research")
  labels   = labels("granary-research", "Granary web page reader for research")
}

target "frontend" {
  context   = "frontend"
  dockerfile = "Dockerfile"
  platforms = ["linux/amd64"]
  tags      = tags("frontend")
  labels    = labels("granary-frontend", "Granary web app (Next.js)")
}

target "stt" {
  context   = "services/stt"
  dockerfile = "Dockerfile"
  platforms = ["linux/amd64"]
  tags      = tags("stt")
  labels    = labels("granary-stt", "Granary speech to text (faster-whisper, CPU)")
}
