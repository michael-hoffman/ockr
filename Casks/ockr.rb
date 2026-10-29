cask "ockr" do
  version "0.4.1"
  sha256 "0d6e8d7914311adcccf695cf6aea9ea24f735d317f1fa8963d0646ef76a82c25"

  url "https://github.com/michael-hoffman/ockr/releases/download/v#{version}/ockr-#{version}.dmg"
  name "ockr"
  desc "Typst-native, keyboard-driven note editor with a Helix-style modal interface"
  homepage "https://michael-hoffman.github.io/ockr/"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on macos: :big_sur

  app "ockr.app"

  # ockr isn't notarized yet (no Apple Developer ID) — Homebrew quarantines
  # every download, and an ad-hoc-signed app under quarantine is blocked by
  # Gatekeeper outright rather than just warning. Strip the quarantine
  # attribute post-install so `brew install --cask ockr` produces a directly
  # launchable app instead of requiring a manual right-click → Open.
  postflight do
    system_command "/usr/bin/xattr",
                   args: ["-dr", "com.apple.quarantine", "#{appdir}/ockr.app"],
                   sudo: false
  end

  zap trash: [
    "~/.local/share/ockr",
    "~/Library/Application Support/ockr",
    "~/Library/Preferences/dev.ockr.editor.plist",
    "~/Library/Saved Application State/dev.ockr.editor.savedState",
  ]
end
