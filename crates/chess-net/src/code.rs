//! Game codes like `7-crossword-pilot`.
//!
//! The number (the nameplate) is all the rendezvous server ever sees: it
//! pairs the two players by it. The words stay on the two machines. They
//! will be the SPAKE2 password in the handshake, so a server that knows only
//! the nameplate cannot stand in the middle.

/// 256 words, so each word is one random byte.
pub const WORDS: [&str; 256] = [
    "acid", "acorn", "actor", "adobe", "agent", "alarm", "album", "alpine", "amber", "anchor",
    "angle", "apple", "apron", "arena", "armor", "arrow", "atlas", "attic", "autumn", "badge",
    "bagel", "bakery", "balloon", "bamboo", "banjo", "barley", "barrel", "basil", "basket",
    "beacon", "beaver", "bench", "berry", "bicycle", "bishop", "blanket", "blossom", "bonfire",
    "bottle", "bracket", "breeze", "brick", "bridge", "bronze", "bubble", "bucket", "bugle",
    "butter", "cabin", "cactus", "camel", "candle", "canoe", "canyon", "captain", "carpet",
    "castle", "cedar", "cello", "chalk", "cherry", "chimney", "cinder", "circus", "citrus",
    "clover", "cobalt", "cocoa", "comet", "compass", "copper", "coral", "cotton", "cougar",
    "crayon", "cricket", "crystal", "dagger", "daisy", "dancer", "delta", "denim", "desert",
    "diamond", "dolphin", "donkey", "dragon", "drum", "eagle", "easel", "echo", "eclipse", "elbow",
    "ember", "engine", "falcon", "feather", "fennel", "ferry", "fiddle", "fig", "flint", "forest",
    "fossil", "fountain", "fox", "galaxy", "garden", "garlic", "gazelle", "geyser", "ginger",
    "glacier", "globe", "goblet", "granite", "grape", "gravel", "guitar", "hammer", "harbor",
    "harvest", "hazel", "helmet", "heron", "honey", "hornet", "husky", "igloo", "island", "ivory",
    "jacket", "jaguar", "jasmine", "jelly", "jungle", "kayak", "kettle", "kitten", "koala",
    "ladder", "lagoon", "lantern", "lemon", "lentil", "lily", "locket", "lotus", "magnet", "mango",
    "maple", "marble", "meadow", "melon", "meteor", "mirror", "mitten", "mosaic", "muffin",
    "mural", "nectar", "needle", "nickel", "noodle", "nutmeg", "oasis", "ocean", "olive", "onion",
    "orbit", "orchid", "otter", "oyster", "paddle", "pancake", "panda", "parrot", "peanut",
    "pebble", "pepper", "piano", "pilot", "pine", "planet", "plum", "pocket", "pony", "poplar",
    "potato", "prism", "pumpkin", "puzzle", "quartz", "quill", "rabbit", "radar", "radish",
    "raven", "reef", "ribbon", "river", "robin", "rocket", "rose", "ruby", "saddle", "salmon",
    "sandal", "satin", "scarf", "shadow", "shell", "sierra", "silver", "sketch", "sled", "sonnet",
    "spark", "spider", "spruce", "squash", "statue", "storm", "sugar", "summit", "sunset", "swan",
    "tango", "teapot", "temple", "thistle", "thunder", "tiger", "timber", "toast", "tomato",
    "topaz", "tractor", "trumpet", "tulip", "tundra", "turtle", "valley", "velvet", "violin",
    "volcano", "waffle", "walnut", "walrus", "willow", "window", "wizard", "yarn", "yogurt",
    "zebra", "zephyr",
];

/// A fresh code for `nameplate`, with two random words.
pub fn new_code(nameplate: u16) -> String {
    let mut bytes = [0u8; 2];
    getrandom::fill(&mut bytes).expect("no system randomness");
    format!(
        "{nameplate}-{}-{}",
        WORDS[bytes[0] as usize], WORDS[bytes[1] as usize]
    )
}

/// The nameplate of a typed code, and its words (lower-cased). Rejects
/// codes whose words are not in the list.
pub fn parse_code(code: &str) -> Option<(u16, String)> {
    let code = code.trim().to_ascii_lowercase();
    let (nameplate, words) = code.split_once('-')?;
    let nameplate = nameplate.parse().ok()?;
    let mut count = 0;
    for word in words.split('-') {
        WORDS.binary_search(&word).ok()?;
        count += 1;
    }
    (count == 2).then(|| (nameplate, words.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_sorted_and_unique() {
        assert!(WORDS.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn codes_round_trip() {
        let code = new_code(7);
        let (nameplate, words) = parse_code(&code).unwrap();
        assert_eq!(nameplate, 7);
        assert_eq!(format!("7-{words}"), code);
        assert_eq!(
            parse_code(" 12-Comet-Pilot ").unwrap(),
            (12, "comet-pilot".into())
        );
        assert_eq!(parse_code("7-comet"), None);
        assert_eq!(parse_code("7-comet-pilot-otter"), None);
        assert_eq!(parse_code("7-comet-notaword"), None);
        assert_eq!(parse_code("x-comet-pilot"), None);
    }
}
