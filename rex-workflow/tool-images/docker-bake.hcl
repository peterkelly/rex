variable "ALPINE_IMAGE" {
  default = "alpine:3.24@sha256:28bd5fe8b56d1bd048e5babf5b10710ebe0bae67db86916198a6eec434943f8b"
}

variable "IMAGE_PREFIX" {
  default = "rex-tool"
}

variable "IMAGE_TAG" {
  default = "local"
}

group "default" {
  targets = ["ffmpeg", "graphviz", "imagemagick", "qpdf", "poppler"]
}

target "common" {
  args = {
    ALPINE_IMAGE = ALPINE_IMAGE
  }
}

target "ffmpeg" {
  inherits   = ["common"]
  context    = "./ffmpeg"
  dockerfile = "Dockerfile"
  tags       = ["${IMAGE_PREFIX}-ffmpeg:${IMAGE_TAG}"]
}

target "graphviz" {
  inherits   = ["common"]
  context    = "./graphviz"
  dockerfile = "Dockerfile"
  tags       = ["${IMAGE_PREFIX}-graphviz:${IMAGE_TAG}"]
}

target "imagemagick" {
  inherits   = ["common"]
  context    = "./imagemagick"
  dockerfile = "Dockerfile"
  tags       = ["${IMAGE_PREFIX}-imagemagick:${IMAGE_TAG}"]
}

target "qpdf" {
  inherits   = ["common"]
  context    = "./qpdf"
  dockerfile = "Dockerfile"
  tags       = ["${IMAGE_PREFIX}-qpdf:${IMAGE_TAG}"]
}

target "poppler" {
  inherits   = ["common"]
  context    = "./poppler"
  dockerfile = "Dockerfile"
  tags       = ["${IMAGE_PREFIX}-poppler:${IMAGE_TAG}"]
}
