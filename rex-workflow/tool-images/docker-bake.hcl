variable "ALPINE_VERSION" {
  default = "3.24"
}

variable "IMAGE_PREFIX" {
  default = "rex-tool"
}

variable "IMAGE_TAG" {
  default = "local"
}

group "default" {
  targets = ["ffmpeg", "imagemagick", "qpdf", "poppler"]
}

target "common" {
  args = {
    ALPINE_VERSION = ALPINE_VERSION
  }
}

target "ffmpeg" {
  inherits   = ["common"]
  context    = "./ffmpeg"
  dockerfile = "Dockerfile"
  tags       = ["${IMAGE_PREFIX}-ffmpeg:${IMAGE_TAG}"]
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
