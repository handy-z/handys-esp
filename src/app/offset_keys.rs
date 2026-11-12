use handy_sdk::roblox::offsets::{OffsetKeys, SDK_KEYS};
use handy_sdk::roblox::read::join_keys;
use handy_sdk::roblox::screen::SCREEN_KEYS;

/// Every offset the app reads; the Updater rejects a dump missing any. Includes SDK_KEYS and SCREEN_KEYS.
pub fn required_keys() -> OffsetKeys {
    join_keys(
        &[&SDK_KEYS, &SCREEN_KEYS],
        &[
            "DataModel.Workspace",
            "DataModel.GameId",
            "BasePart.Primitive",
            "Primitive.Position",
            "Primitive.Size",
            "Model.PrimaryPart",
            "Player.LocalPlayer",
            "Player.ModelInstance",
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_keys_cover_sdk_and_screen_keys() {
        let k = required_keys();
        for key in SDK_KEYS.iter().chain(&SCREEN_KEYS) {
            assert!(k.contains(key), "{key}");
        }
    }
}
