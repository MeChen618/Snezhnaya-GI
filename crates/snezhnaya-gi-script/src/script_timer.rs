use crate::SceneGroupRegistry;
use crate::script_load::GroupLoadState;
use bevy_ecs::prelude::*;
use snezhnaya_gi_data::scene::{EventType, LuaEvt, ScriptCommand};
use snezhnaya_gi_event::lua::{LuaTriggerEvent, ScriptCommandEvent};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const MAX_TIMERS: usize = 1024;
const MAX_TIME_POINTS: usize = 1024;
const MAX_EVENTS_PER_TICK: usize = 256;

pub(crate) fn timer_delay(name: &str, seconds: f64) -> Option<Duration> {
    if name.is_empty() || name.len() > 256 || !(0.0..=31_536_000.0).contains(&seconds) {
        return None;
    }
    Duration::try_from_secs_f64(seconds).ok()
}

pub(crate) fn time_points(name: &str, points: &[f64], repeating: bool) -> Option<Vec<Duration>> {
    if points.is_empty() || points.len() > MAX_TIME_POINTS {
        return None;
    }
    let points = points
        .iter()
        .map(|p| timer_delay(name, *p))
        .collect::<Option<Vec<_>>>()?;
    if points.windows(2).any(|pair| pair[0] > pair[1])
        || (repeating && *points.last()? < Duration::from_millis(1))
    {
        return None;
    }
    Some(points)
}

struct TimeAxis {
    start: Instant,
    points: Vec<Duration>,
    next: usize,
    repeating: bool,
    paused_at: Option<Instant>,
}

impl TimeAxis {
    fn deadline(&self) -> Option<Instant> {
        self.paused_at
            .is_none()
            .then(|| self.start.checked_add(self.points[self.next]))
            .flatten()
    }

    fn advance(&mut self) -> bool {
        self.next += 1;
        if self.next < self.points.len() {
            return true;
        }
        if !self.repeating {
            return false;
        }
        let Some(start) = self.start.checked_add(*self.points.last().unwrap()) else {
            return false;
        };
        self.start = start;
        self.next = 0;
        true
    }
}

#[derive(Resource, Default)]
pub(crate) struct ScriptTimers {
    timers: BTreeMap<(u32, String), Instant>,
    axes: BTreeMap<(u32, String), TimeAxis>,
}

impl ScriptTimers {
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.timers
            .values()
            .copied()
            .chain(self.axes.values().filter_map(TimeAxis::deadline))
            .min()
    }

    pub(crate) fn apply(&mut self, command: &ScriptCommand, now: Instant) {
        match command {
            ScriptCommand::CreateGroupTimerEvent {
                group_id,
                source,
                time,
            } => {
                let key = (*group_id, source.clone());
                if self.timers.contains_key(&key)
                    || self.timers.len() + self.axes.len() < MAX_TIMERS
                {
                    if let Some(deadline) =
                        timer_delay(source, *time).and_then(|d| now.checked_add(d))
                    {
                        self.timers.insert(key, deadline);
                    }
                }
            }
            ScriptCommand::CancelGroupTimerEvent { group_id, source } => {
                self.timers.remove(&(*group_id, source.clone()));
            }
            ScriptCommand::InitTimeAxis {
                group_id,
                identifier,
                delays,
                should_loop,
            } => {
                let key = (*group_id, identifier.clone());
                if self.axes.contains_key(&key) || self.timers.len() + self.axes.len() < MAX_TIMERS
                {
                    if let Some(points) = time_points(identifier, delays, *should_loop) {
                        self.axes.insert(
                            key,
                            TimeAxis {
                                start: now,
                                points,
                                next: 0,
                                repeating: *should_loop,
                                paused_at: None,
                            },
                        );
                    }
                }
            }
            ScriptCommand::EndTimeAxis {
                group_id,
                identifier,
            } => {
                self.axes.remove(&(*group_id, identifier.clone()));
            }
            ScriptCommand::EndAllTimeAxis { group_id } => {
                self.axes.retain(|(group, _), _| group != group_id);
            }
            ScriptCommand::SetTimeAxisPaused {
                group_id,
                identifier,
                paused,
            } => {
                if let Some(axis) = self.axes.get_mut(&(*group_id, identifier.clone())) {
                    if *paused {
                        axis.paused_at.get_or_insert(now);
                    } else if let Some(since) = axis.paused_at.take() {
                        if let Some(start) =
                            axis.start.checked_add(now.saturating_duration_since(since))
                        {
                            axis.start = start;
                        }
                    }
                }
            }
            ScriptCommand::UnloadGroup { group_id }
            | ScriptCommand::RefreshGroup { group_id, .. }
            | ScriptCommand::GoToGroupSuite { group_id, .. } => {
                self.timers.retain(|(group, _), _| group != group_id);
                self.axes.retain(|(group, _), _| group != group_id);
            }
            _ => {}
        }
    }

    pub(crate) fn drain_due(&mut self, now: Instant) -> Vec<LuaTriggerEvent> {
        let mut events = Vec::new();
        // 一轮限制数量，避免循环时间轴积压后独占逻辑线程。
        while events.len() < MAX_EVENTS_PER_TICK {
            let timer = self.timers.iter().min_by_key(|(_, deadline)| *deadline);
            let axis = self
                .axes
                .iter()
                .filter_map(|(key, axis)| Some((key, axis.deadline()?)))
                .min_by_key(|(_, deadline)| *deadline);
            let (key, deadline, is_axis) = match (timer, axis) {
                (Some((_, deadline)), Some((axis_key, axis_deadline)))
                    if axis_deadline < *deadline =>
                {
                    (axis_key.clone(), axis_deadline, true)
                }
                (Some((key, deadline)), _) => (key.clone(), *deadline, false),
                (None, Some((key, deadline))) => (key.clone(), deadline, true),
                (None, None) => break,
            };
            if deadline > now {
                break;
            }
            let index = if is_axis {
                let axis = self.axes.get_mut(&key).unwrap();
                let index = axis.next + 1;
                if !axis.advance() {
                    self.axes.remove(&key);
                }
                index as i64
            } else {
                self.timers.remove(&key);
                0
            };
            events.push(LuaTriggerEvent {
                group_id: key.0,
                event_type: if is_axis {
                    EventType::EventTimeAxisPass
                } else {
                    EventType::EventTimerEvent
                },
                evt: LuaEvt {
                    param1: index,
                    source_name: key.1.clone(),
                    param_str1: key.1,
                    ..Default::default()
                },
            });
        }
        events
    }
}

pub(crate) fn script_timer_system(
    mut commands: MessageReader<ScriptCommandEvent>,
    mut timers: ResMut<ScriptTimers>,
    registry: NonSend<SceneGroupRegistry>,
    mut events: MessageWriter<LuaTriggerEvent>,
) {
    let now = Instant::now();
    for event in commands.read() {
        let group_id = match &event.command {
            ScriptCommand::CreateGroupTimerEvent { group_id, .. }
            | ScriptCommand::InitTimeAxis { group_id, .. } => Some(*group_id),
            _ => None,
        };
        if group_id
            .is_some_and(|id| !matches!(registry.groups.get(&id), Some(GroupLoadState::Loaded(_))))
        {
            continue;
        }
        timers.apply(&event.command, now);
    }
    timers
        .timers
        .retain(|(id, _), _| matches!(registry.groups.get(id), Some(GroupLoadState::Loaded(_))));
    timers
        .axes
        .retain(|(id, _), _| matches!(registry.groups.get(id), Some(GroupLoadState::Loaded(_))));
    for event in timers.drain_due(now) {
        if matches!(
            registry.groups.get(&event.group_id),
            Some(GroupLoadState::Loaded(_))
        ) {
            events.write(event);
        }
    }
}
