# Piece sprites

`chess-pieces-static.png` is "Chess Asset 16bit pixel art" by Graciano Choc (gchoc / bz.gamedev), from https://hollowsmilegames.itch.io/pixel-art-chess-set, dedicated to the public domain under CC0 1.0 Universal.

The sheet is 6 columns of 16x16 sprites (rook, knight, bishop, queen, king, pawn) by 5 colour variants. The game uses row 0 for White and row 1 for Black.

The game does not read this PNG at runtime. After changing it, regenerate the embedded data:

    cargo run --example import_sprites

That rewrites `crates/chess-tui/src/sprite_sheet.rs`.
