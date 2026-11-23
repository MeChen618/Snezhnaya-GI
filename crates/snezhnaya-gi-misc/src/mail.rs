use bevy_ecs::prelude::*;
use snezhnaya_gi_inventory::transaction::{self as inventory, Amounts};
use snezhnaya_gi_message::{event::ClientMessageEvent, output::MessageOutput};
use snezhnaya_gi_persistence::Players;
use snezhnaya_gi_proto::{normal as n, retcode::Retcode, server_only as b};
use std::collections::HashSet;

fn now() -> u32 {
    common::time_util::unix_timestamp().min(u64::from(u32::MAX)) as u32
}
fn client(mail: &b::MailData) -> n::MailData {
    n::MailData {
        mail_id: mail.mail_id,
        mail_text_content: mail.mail_text_content.as_ref().map(|t| n::MailTextContent {
            title: t.title.clone(),
            content: t.content.clone(),
            sender: t.sender.clone(),
        }),
        item_list: mail
            .item_list
            .iter()
            .map(|i| n::MailItem {
                equip_param: i.equip_param.as_ref().map(|p| n::EquipParam {
                    item_id: p.item_id,
                    item_num: p.item_num,
                    item_level: p.item_level,
                    promote_level: p.promote_level,
                }),
                ..Default::default()
            })
            .collect(),
        send_time: mail.send_time,
        expire_time: mail.expire_time,
        importance: mail.importance,
        is_read: mail.is_read,
        is_attachment_got: mail.is_attachment_got,
        config_id: mail.config_id,
        argument_list: mail.argument_list.clone(),
        collect_state: mail.collect_state,
    }
}
fn available(mail: &b::MailData, time: u32) -> bool {
    mail.send_time <= time
        && (mail.expire_time == 0 || mail.expire_time > time || mail.collect_state == 3)
}
fn notify(
    player: &b::PlayerDataBin,
    out: &MessageOutput,
    changed: &[u32],
    deleted: Vec<u32>,
    new: bool,
) {
    let Some(state) = &player.mail_bin else {
        return;
    };
    let list: Vec<_> = changed
        .iter()
        .filter_map(|id| state.mail_map.get(id))
        .filter(|mail| available(mail, now()))
        .map(client)
        .collect();
    if list.is_empty() && deleted.is_empty() {
        return;
    }
    let response = if new {
        n::MailChangeNotify {
            mail_list: list,
            del_mail_id_list: deleted,
            ..Default::default()
        }
    } else {
        n::MailChangeNotify {
            change_mail_list: list,
            del_mail_id_list: deleted,
            ..Default::default()
        }
    };
    out.send(player.uid, "MailChangeNotify", response);
}
fn refresh(player: &mut b::PlayerDataBin, out: &MessageOutput) {
    let time = now();
    let state = player.mail_bin.get_or_insert_with(Default::default);
    state.mail_id_counter = state
        .mail_id_counter
        .max(state.mail_map.keys().copied().max().unwrap_or(0))
        .max(
            state
                .has_take_attachment_mail_id_list
                .iter()
                .copied()
                .max()
                .unwrap_or(0),
        );
    let deleted: Vec<_> = state
        .mail_map
        .iter()
        .filter(|(_, m)| m.expire_time != 0 && m.expire_time <= time && m.collect_state != 3)
        .map(|(&id, _)| id)
        .collect();
    for id in &deleted {
        state.mail_map.remove(id);
    }
    let mut pending: Vec<_> = state
        .wait_add_mail_map
        .iter()
        .map(|(id, m)| (id.to_string(), m.clone(), true))
        .chain(
            state
                .new_wait_add_mail_map
                .iter()
                .map(|(id, m)| (id.clone(), m.clone(), false)),
        )
        .collect();
    pending.sort_by(|a, b| a.0.cmp(&b.0));
    let limits = snezhnaya_gi_data::excel::production::ProductionConfig::get()
        .constants
        .get("CONST_VALUE_MAIL_MAX_NUM");
    let mut added = Vec::new();
    for (key, mail, legacy) in pending {
        let source = key.parse::<u32>().ok().filter(|id| *id != 0);
        let receipt = source
            .map(|id| format!("id:{id}"))
            .unwrap_or_else(|| format!("key:{key}"));
        if state.mail_source_id_map.contains_key(&receipt) {
            if legacy {
                state.wait_add_mail_map.remove(&key.parse().unwrap());
            } else {
                state.new_wait_add_mail_map.remove(&key);
            }
            continue;
        }
        let collected = mail.collect_state == 3;
        let max = limits
            .and_then(|v| v.get(usize::from(collected)))
            .and_then(|s| s.parse().ok())
            .unwrap_or(1000usize);
        let used = state
            .mail_map
            .values()
            .filter(|m| (m.collect_state == 3) == collected)
            .count();
        if used >= max {
            continue;
        }
        let Some(id) = state.mail_id_counter.checked_add(1) else {
            break;
        };
        let already_claimed = source.is_some_and(|old| {
            state.has_take_attachment_mail_id_list.contains(&old)
                && !state.mail_source_id_map.values().any(|value| *value == old)
        });
        state.mail_id_counter = id;
        state.mail_source_id_map.insert(receipt, id);
        let data = b::MailData {
            mail_id: id,
            mail_text_content: Some(b::MailTextContent {
                title: mail.title,
                content: mail.content,
                sender: mail.sender,
            }),
            item_list: mail
                .item_param_list
                .into_iter()
                .filter(|i| i.item_id != 0 && i.count != 0)
                .map(|i| b::MailItem {
                    equip_param: Some(b::EquipParam {
                        item_id: i.item_id,
                        item_num: i.count,
                        item_level: i.level,
                        promote_level: i.promote_level,
                    }),
                    ..Default::default()
                })
                .collect(),
            send_time: if mail.send_time == 0 {
                time
            } else {
                mail.send_time
            },
            expire_time: mail.expire_time,
            importance: mail.importance,
            is_attachment_got: already_claimed,
            config_id: mail.config_id,
            argument_list: mail.argument_list,
            collect_state: if mail.collect_state == 0 {
                1
            } else {
                mail.collect_state as i32
            },
            ..Default::default()
        };
        if data.expire_time == 0 || data.expire_time > time || data.collect_state == 3 {
            state.mail_map.insert(id, data);
            added.push(id);
        }
        if legacy {
            state.wait_add_mail_map.remove(&key.parse().unwrap());
        } else {
            state.new_wait_add_mail_map.remove(&key);
        }
    }
    if !added.is_empty() || !deleted.is_empty() {
        notify(player, out, &added, deleted, true);
    }
}
pub fn login(mut players: ResMut<Players>, out: Res<MessageOutput>) {
    for player in players.values_mut() {
        refresh(player, &out);
    }
}
fn list(player: &b::PlayerDataBin, collected: bool) -> Vec<n::MailData> {
    let mut mails: Vec<_> = player
        .mail_bin
        .iter()
        .flat_map(|s| s.mail_map.values())
        .filter(|m| (m.collect_state == 3) == collected && available(m, now()))
        .map(client)
        .collect();
    mails.sort_by_key(|m| std::cmp::Reverse((m.importance, m.send_time, m.mail_id)));
    mails
}
fn validate_ids(player: &b::PlayerDataBin, ids: &[u32]) -> Result<(), Retcode> {
    if ids.is_empty() || ids.len() > 1000 {
        return Err(Retcode::RetMailParaErr);
    }
    let mut seen = HashSet::new();
    let state = player.mail_bin.as_ref().ok_or(Retcode::RetMailParaErr)?;
    for id in ids {
        if !seen.insert(*id) || !state.mail_map.contains_key(id) {
            return Err(Retcode::RetMailParaErr);
        }
    }
    Ok(())
}
fn claim(
    player: &mut b::PlayerDataBin,
    ids: &[u32],
    out: &MessageOutput,
) -> Result<n::GetMailItemRsp, Retcode> {
    validate_ids(player, ids)?;
    let state = player.mail_bin.as_ref().unwrap();
    let mut rewards = Amounts::new();
    for id in ids {
        let mail = &state.mail_map[id];
        if !available(mail, now()) {
            return Err(Retcode::RetMailExpired);
        }
        if mail.is_attachment_got {
            return Err(Retcode::RetRewardHasTaken);
        }
        for item in &mail.item_list {
            let p = item.equip_param.as_ref().ok_or(Retcode::RetMailParaErr)?;
            if p.item_level > 1 || p.promote_level != 0 || item.delete_info.is_some() {
                return Err(Retcode::RetNotSupportItem);
            }
            inventory::add_amount(&mut rewards, p.item_id, p.item_num)?;
        }
    }
    // 所有附件一起入包，成功后才更新邮件的领取状态。
    let change = inventory::transact(player, &Amounts::new(), &rewards, &[])?;
    let state = player.mail_bin.as_mut().unwrap();
    for id in ids {
        state.mail_map.get_mut(id).unwrap().is_attachment_got = true;
        if !state.has_take_attachment_mail_id_list.contains(id) {
            state.has_take_attachment_mail_id_list.push(*id);
        }
    }
    inventory::notify_transaction(out, player, &change);
    notify(player, out, ids, vec![], false);
    Ok(n::GetMailItemRsp {
        mail_id_list: ids.to_vec(),
        item_list: rewards
            .into_iter()
            .map(|(item_id, item_num)| n::EquipParam {
                item_id,
                item_num,
                item_level: u32::from(
                    snezhnaya_gi_data::excel::weapon_excel_config_collection::get()
                        .contains_key(&item_id)
                        || snezhnaya_gi_data::excel::reliquary_excel_config_collection::get()
                            .contains_key(&item_id),
                ),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    })
}
fn delete(
    player: &mut b::PlayerDataBin,
    ids: &[u32],
    out: &MessageOutput,
) -> Result<n::DelMailRsp, Retcode> {
    validate_ids(player, ids)?;
    let state = player.mail_bin.as_mut().unwrap();
    for id in ids {
        let mail = &state.mail_map[id];
        if mail.collect_state == 3 {
            return Err(Retcode::RetMailDeleteCollected);
        }
        if !mail.item_list.is_empty() && !mail.is_attachment_got {
            return Err(Retcode::RetMailItemNotGet);
        }
    }
    for id in ids {
        state.mail_map.remove(id);
    }
    notify(player, out, &[], ids.to_vec(), false);
    Ok(n::DelMailRsp {
        mail_id_list: ids.to_vec(),
        ..Default::default()
    })
}
pub fn handle(
    mut events: MessageReader<ClientMessageEvent>,
    mut players: ResMut<Players>,
    out: Res<MessageOutput>,
) {
    for message in events.read() {
        if !matches!(
            message.message_name(),
            "GetAllMailReq"
                | "GetMailItemReq"
                | "DelMailReq"
                | "ReadMailNotify"
                | "ChangeMailStarNotify"
        ) {
            continue;
        }
        let Some(player) = players.get_mut(message.sender_uid()) else {
            continue;
        };
        refresh(player, &out);
        match message.message_name() {
            "GetAllMailReq" => {
                let Some(req) = message.decode_json() else {
                    continue;
                };
                let collected = req
                    .get("is_collected")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                out.send_json(player.uid,"GetAllMailRsp",serde_json::json!({"mail_list":list(player,collected),"is_collected":collected,"is_truncated":false}));
            }
            "GetMailItemReq" => {
                let Some(req) = message.decode::<n::GetMailItemReq>() else {
                    continue;
                };
                let rsp = claim(player, &req.mail_id_list, &out).unwrap_or_else(|code| {
                    n::GetMailItemRsp {
                        retcode: code.into(),
                        ..Default::default()
                    }
                });
                out.send(player.uid, "GetMailItemRsp", rsp);
            }
            "DelMailReq" => {
                let Some(req) = message.decode::<n::DelMailReq>() else {
                    continue;
                };
                let rsp =
                    delete(player, &req.mail_id_list, &out).unwrap_or_else(|code| n::DelMailRsp {
                        retcode: code.into(),
                        ..Default::default()
                    });
                out.send(player.uid, "DelMailRsp", rsp);
            }
            "ReadMailNotify" => {
                let Some(req) = message.decode::<n::ReadMailNotify>() else {
                    continue;
                };
                let state = player.mail_bin.as_mut().unwrap();
                let mut changed = Vec::new();
                for id in req.mail_id_list {
                    if let Some(mail) = state.mail_map.get_mut(&id) {
                        if available(mail, now()) && !mail.is_read {
                            mail.is_read = true;
                            changed.push(id);
                        }
                    }
                }
                if !changed.is_empty() {
                    notify(player, &out, &changed, vec![], false);
                }
            }
            "ChangeMailStarNotify" => {
                let Some(req) = message.decode::<n::ChangeMailStarNotify>() else {
                    continue;
                };
                let state = player.mail_bin.as_mut().unwrap();
                let mut changed = Vec::new();
                for id in req.mail_id_list {
                    if let Some(mail) = state
                        .mail_map
                        .get_mut(&id)
                        .filter(|mail| available(mail, now()))
                    {
                        mail.importance = u32::from(req.is_star);
                        changed.push(id);
                    }
                }
                if !changed.is_empty() {
                    notify(player, &out, &changed, vec![], false);
                }
            }
            _ => (),
        }
    }
}
