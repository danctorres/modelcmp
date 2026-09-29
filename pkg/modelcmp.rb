# Homebrew formula template. The release workflow fills in @VERSION@ and the
# @SHA256_<target>@ placeholders and pushes it to danctorres/homebrew-tap.
class Modelcmp < Formula
  desc "Compare models, get recommendations or choose one per task"
  homepage "https://github.com/danctorres/modelcmp"
  license "MIT"

  base = "https://github.com/danctorres/modelcmp/releases/download/v@VERSION@/modelcmp"
  on_macos do
    on_arm do
      url "#{base}-aarch64-apple-darwin-v@VERSION@.tar.gz"
      sha256 "@SHA256_aarch64-apple-darwin@"
    end
    on_intel do
      url "#{base}-x86_64-apple-darwin-v@VERSION@.tar.gz"
      sha256 "@SHA256_x86_64-apple-darwin@"
    end
  end
  on_linux do
    on_arm do
      url "#{base}-aarch64-unknown-linux-musl-v@VERSION@.tar.gz"
      sha256 "@SHA256_aarch64-unknown-linux-musl@"
    end
    on_intel do
      url "#{base}-x86_64-unknown-linux-musl-v@VERSION@.tar.gz"
      sha256 "@SHA256_x86_64-unknown-linux-musl@"
    end
  end

  def install
    bin.install "modelcmp"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/modelcmp --version")
  end
end
