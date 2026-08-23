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
  targets = ["ffmpeg", "gnuplot", "graphviz", "imagemagick", "qpdf", "poppler"]
}

target "common" {
  args = {
    ALPINE_IMAGE = ALPINE_IMAGE
  }
}

target "ffmpeg" {
  inherits   = ["common"]
  context    = "./rex-tool-ffmpeg"
  dockerfile = "Dockerfile"
  tags       = ["${IMAGE_PREFIX}-ffmpeg:${IMAGE_TAG}"]
}

target "gnuplot" {
  inherits   = ["common"]
  context    = "./rex-tool-gnuplot"
  dockerfile = "Dockerfile"
  tags       = ["${IMAGE_PREFIX}-gnuplot:${IMAGE_TAG}"]
}

target "graphviz" {
  inherits   = ["common"]
  context    = "./rex-tool-graphviz"
  dockerfile = "Dockerfile"
  tags       = ["${IMAGE_PREFIX}-graphviz:${IMAGE_TAG}"]
}

target "imagemagick" {
  inherits   = ["common"]
  context    = "./rex-tool-imagemagick"
  dockerfile = "Dockerfile"
  tags       = ["${IMAGE_PREFIX}-imagemagick:${IMAGE_TAG}"]
}

target "qpdf" {
  inherits   = ["common"]
  context    = "./rex-tool-qpdf"
  dockerfile = "Dockerfile"
  tags       = ["${IMAGE_PREFIX}-qpdf:${IMAGE_TAG}"]
}

target "poppler" {
  inherits   = ["common"]
  context    = "./rex-tool-poppler"
  dockerfile = "Dockerfile"
  tags       = ["${IMAGE_PREFIX}-poppler:${IMAGE_TAG}"]
}
