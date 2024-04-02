use crate::context::AppContext;
use crate::util::*;

use std::fs::File;
use std::io::Write as _;
use std::path::PathBuf;
use tera::Tera;

pub struct HtmlRenderer {
    engine: Tera,
    output_dir: PathBuf,
}

impl HtmlRenderer {
    pub fn new(ctx: &AppContext) -> Result<HtmlRenderer> {
        let mut engine =
            Tera::new(format!("{}/**/*_template.html", ctx.input_dir.to_str().unwrap()).as_str())?;
        engine.autoescape_on(vec!["html"]);

        log::info!("Loaded templates:");
        for template in engine.get_template_names() {
            log::info!("{template}");
        }

        Ok(HtmlRenderer {
            engine,
            output_dir: ctx.output_dir.clone(),
        })
    }

    pub fn render(&self, template_name: &str, context: &tera::Context) -> Result<String> {
        match self.engine.render(template_name, context) {
            Ok(rendered) => Ok(rendered),
            Err(e) => Err(e.into()),
        }
    }
    pub fn render_to_file(&self, template_name: &str, context: &tera::Context) -> Result<PathBuf> {
        let rendered = self.render(template_name, context)?;

        let output_name = template_name
            .replace("_template", "")
            .replace("/", "_")
            .replace("\\", "_");

        let output_path = self.output_dir.join(output_name);

        let mut file = File::create(output_path.clone())?; // Use the cloned output_path
        match file.write_all(rendered.as_bytes()) {
            Ok(_) => Ok(output_path),
            Err(e) => Err(e.into()),
        }
    }
}
