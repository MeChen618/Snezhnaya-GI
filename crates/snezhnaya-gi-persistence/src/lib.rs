use std::collections::{hash_map::Keys, HashMap};

use bevy_ecs::prelude::Resource;
use snezhnaya_gi_proto::server_only::PlayerDataBin;

#[derive(Resource)]
pub struct Players(HashMap<u32, PlayerDataBin>);

impl Players {
    pub fn keys(&self) -> Keys<'_, u32, PlayerDataBin> {
        self.0.keys()
    }

    pub fn get(&self, uid: u32) -> Option<&PlayerDataBin>  {
        self.0.get(&uid)
    }

    pub fn get_mut(&mut self, uid: u32) -> Option<&mut PlayerDataBin> {
        self.0.get_mut(&uid)
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut PlayerDataBin> {
        self.0.values_mut()
    }
}

impl From<HashMap<u32, PlayerDataBin>> for Players {
    fn from(value: HashMap<u32, PlayerDataBin>) -> Self {
        Self(value)
    }
}

pub fn player_snapshot(player: &PlayerDataBin) -> std::borrow::Cow<'_, PlayerDataBin> {
    let Some(dungeon) = player
        .dungeon_bin
        .as_ref()
        .filter(|d| d.cur_dungeon_id != 0)
    else {
        return std::borrow::Cow::Borrowed(player);
    };
    let mut snapshot = player.clone();
    // 副本里也保存背包和任务，重登位置回到副本入口。
    if let Some(scene) = snapshot.scene_bin.as_mut() {
        let valid_exit =
            dungeon.quit_scene_id != 0 && dungeon.quit_pos.is_some_and(|p| p.is_valid());
        scene.my_cur_scene_id = if valid_exit { dungeon.quit_scene_id } else { 3 };
        scene.my_cur_scene_pos = if valid_exit {
            dungeon.quit_pos
        } else {
            Some((2336.789, 249.98996, -751.3081).into())
        };
        scene.my_cur_scene_rot = dungeon
            .quit_rot
            .filter(|r| r.is_valid())
            .or_else(|| Some((0.0, 0.0, 0.0).into()));
        scene.cur_scene_owner_uid = player.uid;
    }
    snapshot.dungeon_bin.as_mut().unwrap().cur_dungeon_id = 0;
    std::borrow::Cow::Owned(snapshot)
}
