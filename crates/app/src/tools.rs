use domain::{NewEvent, OperationId, SourceKind};
use services::{
    mechanism::{FOUR_WORDS_PROMPT, FourWords},
    provider::Provider,
    store::Store,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::rc::Rc;
use time::OffsetDateTime;

pub fn cue_model(words: FourWords) -> ModelRc<ui::Cue> {
    Rc::new(VecModel::from(
        words
            .cues
            .into_iter()
            .map(|cue| ui::Cue {
                simple: cue.simple.into(),
                term: cue.term.into(),
                meaning: cue.meaning.into(),
                stage: 0,
            })
            .collect::<Vec<_>>(),
    ))
    .into()
}

pub fn advance(model: ModelRc<ui::Cue>, index: i32) {
    if index < 0 {
        return;
    }
    if let Some(mut cue) = model.row_data(index as usize) {
        cue.stage = (cue.stage + 1) % 3;
        model.set_row_data(index as usize, cue);
    }
}

pub fn four_words() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = services::InstanceGuard::acquire("four_words")?;
    let captured = capture::capture_with(&capture::SystemReader)?;
    let directory = services::data_dir()?;
    let operation = OperationId::new(uuid::Uuid::new_v4().to_string())?;
    let journal_path = directory.join("journal.sqlite3");
    journal::Journal::open(&journal_path)?.append(&[NewEvent::source(
        operation.clone(),
        OffsetDateTime::now_utc(),
        format!("source:{}", operation.as_str()),
        captured.snapshot.clone(),
    )?])?;
    let store = Store::open(directory.join("workspace.sqlite3"))?;
    let provider = store
        .get::<Provider>("provider", "four_words")?
        .or(store.get("provider", "default")?)
        .map(|r| r.value)
        .unwrap_or_default();
    let ui = ui::FourWordsWindow::new()?;
    ui.set_source(captured.snapshot.text.clone().into());
    ui.set_disclosure(
        format!(
            "{} · по кнопке этот фрагмент → {} / {}. Ничего не отправлено автоматически.",
            if captured.snapshot.kind == SourceKind::Selection {
                "Выделение"
            } else {
                "Clipboard fallback: давность и источник не подтверждены; проверьте текст"
            },
            provider.endpoint,
            provider.model
        )
        .into(),
    );
    let source = captured.snapshot.text;
    let weak = ui.as_weak();
    ui.on_generate(move ||{
        let Some(ui)=weak.upgrade() else{return};
        if ui.get_busy(){return;}
        ui.set_busy(true);ui.set_status("Фрагмент уже сохранён. Создаются четыре смысловые опоры…".into());
        let weak=ui.as_weak();let provider=provider.clone();let source=source.clone();let path=journal_path.clone();let operation=operation.clone();
        std::thread::spawn(move ||{
            let result=(||->Result<FourWords,Box<dyn std::error::Error+Send+Sync>>{
                let response=provider.complete(FOUR_WORDS_PROMPT,&source)?;
                journal::Journal::open(path)?.append(&[NewEvent::ai_response(operation,OffsetDateTime::now_utc(),uuid::Uuid::new_v4().to_string(),provider.endpoint,provider.model,response.clone())?])?;
                Ok(FourWords::parse(&response)?)
            })();
            let _=weak.upgrade_in_event_loop(move |ui|{
                ui.set_busy(false);
                match result {
                    Ok(words)=>{ui.set_cues(cue_model(words));ui.set_status("Клик или 1–4: опора → точный термин → контекстный смысл. Это помощь для собственного осмысления, не проверка знания.".into());}
                    Err(error)=>ui.set_status(error.to_string().into()),
                }
            });
        });
    });
    let weak = ui.as_weak();
    ui.on_reveal(move |index| {
        if let Some(ui) = weak.upgrade() {
            advance(ui.get_cues(), index);
        }
    });
    ui.on_cancel(|| {
        let _ = slint::quit_event_loop();
    });
    ui.run()?;
    Ok(())
}
