import re
with open("Cargo.toml", "r") as f:
    text = f.read()

# I want to remove the specific lines defining the workspace dependencies for livekit, libwebrtc, webrtc-sys. Wait actually, I can just leave them in Cargo.toml but as `{ version = "0.0.0" }` or something so they resolve locally? No, let's just make webrtc-sys and libwebrtc dummy crates instead!
