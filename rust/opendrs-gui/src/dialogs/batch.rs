//! 批次新增頻道 (the legacy CPS's ADD++; port of `batch_dialog.py`): N copies of a channel with
//! stepped frequencies and numbered names.

use eframe::egui;
use opendrs_core::device::{Band, frequency_problem_hz};
use opendrs_core::f136::Project;
use opendrs_core::layout::{BatchPlan, MAX_BATCH, zone_rows};
use opendrs_core::pystr::{decimal_value, parse_float, strip};
use opendrs_core::records::mhz_to_hz;
use opendrs_core::textcodec::NameCodec;

/// `f"{hz / 1_000_000:.5f}"` for any integer (a planned frequency can be out of range).
fn mhz_text(hz: i64) -> String {
    format!("{:.5}", hz as f64 / 1_000_000.0)
}

/// "FRS_TW_14" → ("FRS_TW_", 15): continue the numbering after the template.
pub fn split_name(name: &str) -> (String, u32) {
    let digits_start = name
        .char_indices()
        .rev()
        .take_while(|(_, ch)| decimal_value(*ch).is_some())
        .last()
        .map(|(at, _)| at);
    match digits_start {
        Some(at) => {
            let number = name[at..]
                .chars()
                .filter_map(decimal_value)
                .fold(0u64, |n, d| {
                    n.saturating_mul(10).saturating_add(u64::from(d))
                });
            (
                name[..at].to_string(),
                u32::try_from(number.saturating_add(1)).unwrap_or(u32::MAX),
            )
        }
        None => (name.to_string(), 1),
    }
}

/// Problems of the planned channels, 「第 k 台接收：…」.
pub fn plan_problems(plan: &BatchPlan, bands: Option<&[Band]>) -> Vec<String> {
    let mut out = Vec::new();
    for k in 0..plan.count {
        let rx = plan
            .start_rx_hz
            .saturating_add((k as i64).saturating_mul(plan.step_hz));
        for (label, hz) in [("接收", rx), ("發射", rx.saturating_add(plan.tx_offset_hz))] {
            if let Some(reason) = frequency_problem_hz(hz, bands) {
                out.push(format!("第 {} 台{label}：{reason}", k + 1));
            }
        }
    }
    out
}

pub struct BatchDialog {
    pub row: usize,
    zone: String,
    template: String,
    pub count: usize,
    pub start: String,
    pub step: String,
    pub offset: String,
    pub prefix: String,
    pub first: u32,
}

/// What the dialog decided this frame.
pub enum BatchOutcome {
    Open,
    Cancelled,
    Accepted(BatchPlan),
}

impl BatchDialog {
    /// The dialog for template channel `row` of `project`.
    pub fn new(project: &Project, row: usize, codec: NameCodec) -> Self {
        let template = project.channels[row];
        let name = template.name(codec);
        let (prefix, number) = split_name(&name);
        let zone = match zone_rows(project).get(row).copied().flatten() {
            Some(z) => format!("{}. {}", z + 1, project.zones[z].name(codec)),
            None => "1. ?".into(),
        };
        let offset = i64::from(template.tx_hz()) - i64::from(template.rx_hz());
        let offset_text = mhz_text(offset);
        let offset_text = offset_text.trim_end_matches('0').trim_end_matches('.');
        BatchDialog {
            row,
            zone,
            template: name,
            count: 4,
            start: mhz_text(i64::from(template.rx_hz()) + 12_500),
            step: "12.5".into(),
            offset: if offset_text.is_empty() {
                "0".into()
            } else {
                offset_text.into()
            },
            prefix,
            first: number.min(99_999),
        }
    }

    /// The settings entered, or why a number cannot be read.
    pub fn plan(&self) -> Result<BatchPlan, String> {
        let step_text = if strip(&self.step).is_empty() {
            "0"
        } else {
            strip(&self.step)
        };
        let step_khz =
            parse_float(step_text).ok_or_else(|| format!("「{}」不是數字", strip(&self.step)))?;
        let offset = if self.offset.is_empty() {
            "0"
        } else {
            self.offset.as_str()
        };
        Ok(BatchPlan {
            count: self.count,
            start_rx_hz: mhz_to_hz(&self.start).map_err(|e| e.to_string())?,
            step_hz: mhz_to_hz(&format!("{:.6}", step_khz / 1000.0)).map_err(|e| e.to_string())?,
            tx_offset_hz: mhz_to_hz(offset).map_err(|e| e.to_string())?,
            name_prefix: self.prefix.clone(),
            first_number: i64::from(self.first),
        })
    }

    /// The preview line, and whether the plan can be applied.
    pub fn preview(&self, bands: Option<&[Band]>) -> (String, bool) {
        let plan = match self.plan() {
            Ok(plan) => plan,
            Err(e) => return (format!("⚠ {e}"), false),
        };
        let last = plan.count as i64 - 1;
        let rx = |k: i64| {
            plan.start_rx_hz
                .saturating_add(k.saturating_mul(plan.step_hz))
        };
        let mut text = format!(
            "將新增 {} 台：{}{}（接收 {}）… {}{}（接收 {}，發射 {}）",
            plan.count,
            plan.name_prefix,
            plan.first_number,
            mhz_text(rx(0)),
            plan.name_prefix,
            plan.first_number + last,
            mhz_text(rx(last)),
            mhz_text(rx(last).saturating_add(plan.tx_offset_hz))
        );
        let problems = plan_problems(&plan, bands);
        if !problems.is_empty() {
            let shown: Vec<String> = problems.iter().take(3).cloned().collect();
            text.push_str(&format!(
                "\n⚠ {}{}",
                shown.join("；"),
                if problems.len() > 3 { "…" } else { "" }
            ));
        }
        (text, problems.is_empty())
    }

    pub fn show(&mut self, ctx: &egui::Context, bands: Option<&[Band]>) -> BatchOutcome {
        let mut outcome = BatchOutcome::Open;
        let response = egui::Modal::new(egui::Id::new("batch-add")).show(ctx, |ui| {
            ui.set_max_width(520.0);
            ui.heading("批次新增頻道");
            egui::Grid::new("batch-form")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("加入分區");
                    ui.label(&self.zone);
                    ui.end_row();
                    ui.label("範本頻道");
                    ui.label(format!("{}（其餘設定照抄）", self.template));
                    ui.end_row();
                    ui.label("數量");
                    ui.add(egui::DragValue::new(&mut self.count).range(1..=MAX_BATCH));
                    ui.end_row();
                    for (label, text) in [
                        ("起始接收頻率 (MHz)", &mut self.start),
                        ("頻率間隔 (kHz)", &mut self.step),
                        ("發射差頻 (MHz)", &mut self.offset),
                        ("名稱前綴", &mut self.prefix),
                    ] {
                        ui.label(label);
                        ui.text_edit_singleline(text);
                        ui.end_row();
                    }
                    ui.label("起始編號");
                    ui.add(egui::DragValue::new(&mut self.first).range(0..=99_999));
                    ui.end_row();
                });
            let (preview, ok) = self.preview(bands);
            ui.label(preview);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(ok, egui::Button::new("確定")).clicked() {
                    if let Ok(plan) = self.plan() {
                        outcome = BatchOutcome::Accepted(plan);
                    }
                }
                if ui.button("取消").clicked() {
                    outcome = BatchOutcome::Cancelled;
                }
            });
        });
        if matches!(outcome, BatchOutcome::Open) && response.should_close() {
            outcome = BatchOutcome::Cancelled;
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_continue_the_template_numbering() {
        assert_eq!(split_name("FRS_TW_14"), ("FRS_TW_".into(), 15));
        assert_eq!(split_name("HW"), ("HW".into(), 1));
        assert_eq!(split_name("CH１２"), ("CH".into(), 13));
        assert_eq!(split_name("7"), (String::new(), 8));
    }

    #[test]
    fn preview_and_plan_like_python() {
        let dialog = BatchDialog {
            row: 0,
            zone: "4. FRS_TW".into(),
            template: "FRS_TW_14".into(),
            count: 2,
            start: "467.6375".into(),
            step: "12.5".into(),
            offset: "0".into(),
            prefix: "FRS_TW_".into(),
            first: 15,
        };
        let bands = [(136_000_000, 174_000_000), (400_000_000, 480_000_000)];
        assert_eq!(
            dialog.preview(Some(&bands)),
            ("將新增 2 台：FRS_TW_15（接收 467.63750）… FRS_TW_16（接收 467.65000，發射 467.65000）".into(), true)
        );
        let bad = BatchDialog {
            step: "abc".into(),
            ..dialog
        };
        assert_eq!(bad.preview(None), ("⚠ 「abc」不是數字".into(), false));
        let out = BatchDialog {
            start: "479.995".into(),
            step: "10".into(),
            ..bad
        };
        let (text, ok) = out.preview(Some(&bands));
        assert!(
            !ok && text.contains("\n⚠ 第 2 台接收：480.00500 MHz 超出本機頻段"),
            "{text}"
        );
    }
}
