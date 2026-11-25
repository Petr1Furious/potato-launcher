use std::sync::OnceLock;

use gpui_kit::component::{
    ActiveTheme, Disableable as _, Sizable,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputEvent, InputState, NumberInput, NumberInputEvent, NumberStep, StepAction},
    slider::{Slider, SliderEvent, SliderState},
    v_flex,
};
use gpui_kit::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, Subscription, Window,
    div, prelude::*, px, relative,
};
use instance::storage::InstanceHandle;
use launcher_bridge::{BackendSender, InstanceView, MessageToBackend};
use launcher_i18n as t;

const MIN_MB: u64 = 1024;
const STEP_MB: u64 = 512;
const FALLBACK_MB: u64 = 4096;

pub fn system_memory_mb() -> u64 {
    static TOTAL: OnceLock<u64> = OnceLock::new();
    *TOTAL.get_or_init(|| {
        let mut system = sysinfo::System::new();
        system.refresh_memory();
        (system.total_memory() / (1024 * 1024)).max(MIN_MB * 2)
    })
}

fn snap(mb: u64) -> u64 {
    ((mb + STEP_MB / 2) / STEP_MB * STEP_MB).max(MIN_MB)
}

fn format_gb(mb: u64) -> String {
    let gb = mb as f64 / 1024.0;
    if (gb.fract()).abs() < f64::EPSILON {
        format!("{gb:.0}")
    } else {
        format!("{gb:.1}")
    }
}

pub struct MemoryControl {
    sender: BackendSender,
    instance: Option<InstanceHandle>,
    value_mb: u64,
    default_mb: Option<u64>,
    max_mb: u64,
    slider: Entity<SliderState>,
    input: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl MemoryControl {
    pub fn new(sender: BackendSender, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let max_mb = system_memory_mb() / 1024 * 1024;
        let slider = cx.new(|_| {
            SliderState::new()
                .min(MIN_MB as f32)
                .max(max_mb as f32)
                .step(STEP_MB as f32)
                .default_value(FALLBACK_MB as f32)
        });
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_step(None::<NumberStep>, window, cx);
            input
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &slider,
                window,
                |this, _, event: &SliderEvent, window, cx| {
                    match event {
                        SliderEvent::Change(value) => {
                            this.value_mb = value.start() as u64;
                            this.sync_input(window, cx);
                        }
                        SliderEvent::Release(value) => {
                            this.value_mb = value.start() as u64;
                            this.sync_input(window, cx);
                            this.save();
                        }
                    }
                    cx.notify();
                },
            ),
            cx.subscribe_in(
                &input,
                window,
                |this, _, event: &NumberInputEvent, window, cx| {
                    let NumberInputEvent::Step(action) = event;
                    let delta = STEP_MB as i64
                        * match action {
                            StepAction::Increment => 1,
                            StepAction::Decrement => -1,
                        };
                    this.set_value(this.value_mb.saturating_add_signed(delta), window, cx);
                    this.save();
                },
            ),
            cx.subscribe_in(
                &input,
                window,
                |this, input, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        let parsed = input.read(cx).value().trim().replace(',', ".");
                        if let Ok(gb) = parsed.parse::<f64>()
                            && gb.is_finite()
                            && gb > 0.0
                        {
                            let mb = (gb * 1024.0).round() as u64;
                            if mb != this.value_mb {
                                this.set_value(mb, window, cx);
                                this.save();
                                return;
                            }
                        }
                        this.sync_input(window, cx);
                    }
                },
            ),
        ];
        Self {
            sender,
            instance: None,
            value_mb: FALLBACK_MB,
            default_mb: None,
            max_mb,
            slider,
            input,
            _subscriptions: subscriptions,
        }
    }

    /// Picks up the stored value, unless it is the one already shown.
    pub fn sync_from(
        &mut self,
        instance: &InstanceView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = instance
            .effective_xmx_mb
            .or(instance.default_xmx_mb)
            .unwrap_or(FALLBACK_MB);
        let same_instance = self.instance.as_ref() == Some(&instance.handle);
        self.default_mb = instance.default_xmx_mb;
        if same_instance && value == self.value_mb {
            return;
        }
        self.instance = Some(instance.handle.clone());
        self.set_value(value, window, cx);
    }

    fn set_value(&mut self, mb: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.value_mb = snap(mb).min(self.max_mb.max(mb));
        let value = self.value_mb as f32;
        self.slider
            .update(cx, |slider, cx| slider.set_value(value, window, cx));
        self.sync_input(window, cx);
        cx.notify();
    }

    fn sync_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = format_gb(self.value_mb);
        self.input.update(cx, |input, cx| {
            if input.value() != text {
                input.set_value(text, window, cx);
            }
        });
    }

    fn save(&self) {
        let Some(instance) = self.instance.clone() else {
            return;
        };
        let default = self.default_mb.unwrap_or(FALLBACK_MB);
        let xmx_mb = (self.value_mb != default).then_some(self.value_mb);
        self.sender
            .send(MessageToBackend::SetInstanceMemory { instance, xmx_mb });
    }

    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(instance) = self.instance.clone() else {
            return;
        };
        self.set_value(self.default_mb.unwrap_or(FALLBACK_MB), window, cx);
        self.sender.send(MessageToBackend::SetInstanceMemory {
            instance,
            xmx_mb: None,
        });
    }
}

impl Render for MemoryControl {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let total = system_memory_mb();
        let high = self.value_mb * 4 > total * 3;
        let default = self.default_mb.unwrap_or(FALLBACK_MB);
        let is_default = self.value_mb == default;
        let span = (self.max_mb - MIN_MB).max(1) as f32;
        let default_fraction = ((default.saturating_sub(MIN_MB)) as f32 / span).clamp(0.0, 1.0);

        v_flex()
            .w_full()
            .gap_2()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_4()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(Slider::new(&self.slider).w_full())
                            .child(
                                div()
                                    .relative()
                                    .w_full()
                                    .h_4()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(
                                        div()
                                            .absolute()
                                            .left_0()
                                            .child(format!("{} GB", format_gb(MIN_MB))),
                                    )
                                    .child(
                                        div()
                                            .absolute()
                                            .right_0()
                                            .child(format!("{} GB", format_gb(self.max_mb))),
                                    )
                                    .child(
                                        div()
                                            .absolute()
                                            .top_0()
                                            .left(relative(default_fraction))
                                            .ml(px(-1.0))
                                            .w(px(2.0))
                                            .h(px(6.0))
                                            .rounded_full()
                                            .bg(cx.theme().muted_foreground),
                                    ),
                            ),
                    )
                    .child(
                        div().w(px(128.0)).flex_none().child(
                            NumberInput::new(&self.input).small().suffix(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("GB"),
                            ),
                        ),
                    )
                    .child(
                        Button::new("memory-reset")
                            .icon(gpui_kit::assets::IconName::RotateCcw)
                            .ghost()
                            .small()
                            .tooltip(t::memory::reset())
                            .disabled(is_default)
                            .on_click(cx.listener(|this, _, window, cx| this.reset(window, cx))),
                    ),
            )
            .child(div().text_xs().map(|this| {
                if high {
                    this.text_color(cx.theme().warning)
                        .child(t::memory::high_warning(format_gb(total)))
                } else {
                    this.text_color(cx.theme().muted_foreground)
                        .child(t::memory::pack_default(format_gb(default)))
                }
            }))
    }
}
