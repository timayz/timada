//! `/media/demo/{file}`: a drawn stand-in for the product photos the demo
//! does not have — the object it is, in profile, on a near-white plate, so
//! listings and product pages look like a shop's rather than a wireframe's.

use topcoat::{
    Result,
    context::Cx,
    router::{
        error::RouterErrorExt,
        path_param, path_param as param,
        response::{IntoResponse, Response},
        route,
    },
};

path_param!(pub file: String, error = not_found);

pub struct Svg(String);

impl IntoResponse for Svg {
    fn into_response(self, _cx: &Cx) -> Result<Response> {
        Ok(Response::builder()
            .header("Content-Type", "image/svg+xml; charset=utf-8")
            .header("Cache-Control", "public, max-age=86400")
            .body(self.0.into_bytes().into())?)
    }
}

/// What the drawing shows. The route is handed a reference and nothing else —
/// no category, no product — so the object is read out of the reference.
#[derive(Clone, Copy)]
enum Shape {
    Speaker,
    Monitor,
    Keyboard,
    Mouse,
    Headphones,
    Gpu,
    Ssd,
    Lamp,
    Parcel,
}

impl Shape {
    /// The order the fallback cycles through, so an unknown reference still
    /// gets the same object every time.
    const ALL: [Shape; 9] = [
        Shape::Speaker,
        Shape::Monitor,
        Shape::Keyboard,
        Shape::Mouse,
        Shape::Headphones,
        Shape::Gpu,
        Shape::Ssd,
        Shape::Lamp,
        Shape::Parcel,
    ];

    /// First needle the reference contains wins. The needles cover the seeded
    /// references *and* the category slugs, so `/media/demo/ecran-pc.svg`
    /// draws a monitor as well. Order matters: `cru-mx500` is an SSD, not the
    /// `mx`-something keyboard.
    fn of(reference: &str, hue: u32) -> Self {
        const NEEDLES: &[(&str, Shape)] = &[
            ("xm5", Shape::Headphones),
            ("hs80", Shape::Headphones),
            ("casque", Shape::Headphones),
            ("flip", Shape::Speaker),
            ("charge", Shape::Speaker),
            ("enceinte", Shape::Speaker),
            ("image-son", Shape::Speaker),
            ("hue", Shape::Lamp),
            ("lampe", Shape::Lamp),
            ("4070", Shape::Gpu),
            ("4060", Shape::Gpu),
            ("7800", Shape::Gpu),
            ("carte-graphique", Shape::Gpu),
            ("990p", Shape::Ssd),
            ("mx500", Shape::Ssd),
            ("p3-", Shape::Ssd),
            ("ssd", Shape::Ssd),
            ("mxkeys", Shape::Keyboard),
            ("g915", Shape::Keyboard),
            ("k70", Shape::Keyboard),
            ("k55", Shape::Keyboard),
            ("clavier", Shape::Keyboard),
            ("mx3s", Shape::Mouse),
            ("g502", Shape::Mouse),
            ("m65", Shape::Mouse),
            ("souris", Shape::Mouse),
            ("27gp850", Shape::Monitor),
            ("34wp65", Shape::Monitor),
            ("ody", Shape::Monitor),
            ("s24r", Shape::Monitor),
            ("vg249", Shape::Monitor),
            ("q27g2", Shape::Monitor),
            ("ecran", Shape::Monitor),
            // The departments of the tree, so a category tile shows something
            // of its own rather than the fallback's.
            ("informatique", Shape::Monitor),
            ("bureautique", Shape::Keyboard),
            ("peripherique", Shape::Keyboard),
            ("composant", Shape::Gpu),
            ("stockage", Shape::Ssd),
            ("audio", Shape::Headphones),
            ("maison", Shape::Lamp),
        ];
        NEEDLES
            .iter()
            .find(|(needle, _)| reference.contains(needle))
            .map_or_else(
                || Shape::ALL[(hue as usize) % Shape::ALL.len()],
                |(_, shape)| *shape,
            )
    }

    /// The object itself, drawn between y=110 and y=370 so it sits on the
    /// ground shadow with the caption clear underneath. `body` is the lit
    /// face, `detail` the shaded parts, `gloss` the top-left highlight that
    /// sells "photographed on white".
    fn draw(self, body: &str, detail: &str) -> String {
        let gloss = "fill=\"#fff\" opacity=\".3\"";
        match self {
            Shape::Monitor => format!(
                "<rect x=\"92\" y=\"114\" width=\"296\" height=\"182\" rx=\"12\" fill=\"{detail}\"/>\
                 <rect x=\"106\" y=\"128\" width=\"268\" height=\"154\" rx=\"5\" fill=\"{body}\"/>\
                 <rect x=\"120\" y=\"140\" width=\"104\" height=\"58\" rx=\"4\" {gloss}/>\
                 <rect x=\"222\" y=\"296\" width=\"36\" height=\"46\" fill=\"{detail}\"/>\
                 <rect x=\"166\" y=\"342\" width=\"148\" height=\"16\" rx=\"8\" fill=\"{detail}\"/>"
            ),
            Shape::Speaker => format!(
                "<rect x=\"152\" y=\"128\" width=\"176\" height=\"224\" rx=\"88\" fill=\"{body}\"/>\
                 <rect x=\"152\" y=\"214\" width=\"176\" height=\"18\" fill=\"{detail}\" opacity=\".45\"/>\
                 <rect x=\"174\" y=\"152\" width=\"52\" height=\"120\" rx=\"26\" {gloss}/>\
                 <circle cx=\"216\" cy=\"310\" r=\"11\" fill=\"{detail}\"/>\
                 <circle cx=\"264\" cy=\"310\" r=\"11\" fill=\"{detail}\"/>"
            ),
            Shape::Headphones => format!(
                "<path d=\"M126 296a114 114 0 0 1 228 0\" fill=\"none\" stroke=\"{detail}\" stroke-width=\"28\" stroke-linecap=\"round\"/>\
                 <rect x=\"104\" y=\"268\" width=\"58\" height=\"104\" rx=\"27\" fill=\"{body}\"/>\
                 <rect x=\"318\" y=\"268\" width=\"58\" height=\"104\" rx=\"27\" fill=\"{body}\"/>\
                 <rect x=\"118\" y=\"282\" width=\"22\" height=\"48\" rx=\"11\" {gloss}/>"
            ),
            Shape::Keyboard => format!(
                "<rect x=\"78\" y=\"186\" width=\"324\" height=\"140\" rx=\"18\" fill=\"{body}\"/>\
                 <rect x=\"96\" y=\"200\" width=\"126\" height=\"40\" rx=\"10\" {gloss}/>\
                 <rect x=\"100\" y=\"250\" width=\"280\" height=\"18\" rx=\"7\" fill=\"{detail}\" opacity=\".55\"/>\
                 <rect x=\"100\" y=\"276\" width=\"280\" height=\"18\" rx=\"7\" fill=\"{detail}\" opacity=\".55\"/>\
                 <rect x=\"170\" y=\"302\" width=\"140\" height=\"16\" rx=\"7\" fill=\"{detail}\" opacity=\".55\"/>"
            ),
            Shape::Mouse => format!(
                "<rect x=\"176\" y=\"128\" width=\"128\" height=\"228\" rx=\"64\" fill=\"{body}\"/>\
                 <rect x=\"196\" y=\"150\" width=\"40\" height=\"96\" rx=\"20\" {gloss}/>\
                 <rect x=\"236\" y=\"134\" width=\"8\" height=\"84\" rx=\"4\" fill=\"{detail}\" opacity=\".5\"/>\
                 <rect x=\"230\" y=\"162\" width=\"20\" height=\"40\" rx=\"10\" fill=\"{detail}\"/>"
            ),
            Shape::Gpu => format!(
                "<rect x=\"64\" y=\"174\" width=\"352\" height=\"142\" rx=\"14\" fill=\"{detail}\"/>\
                 <rect x=\"64\" y=\"316\" width=\"126\" height=\"24\" rx=\"6\" fill=\"{detail}\" opacity=\".6\"/>\
                 <circle cx=\"156\" cy=\"245\" r=\"50\" fill=\"{body}\"/>\
                 <circle cx=\"156\" cy=\"245\" r=\"13\" fill=\"{detail}\"/>\
                 <circle cx=\"300\" cy=\"245\" r=\"50\" fill=\"{body}\"/>\
                 <circle cx=\"300\" cy=\"245\" r=\"13\" fill=\"{detail}\"/>\
                 <rect x=\"78\" y=\"186\" width=\"56\" height=\"14\" rx=\"7\" {gloss}/>"
            ),
            Shape::Ssd => format!(
                "<rect x=\"84\" y=\"206\" width=\"312\" height=\"76\" rx=\"10\" fill=\"{body}\"/>\
                 <rect x=\"100\" y=\"218\" width=\"88\" height=\"18\" rx=\"6\" {gloss}/>\
                 <rect x=\"116\" y=\"224\" width=\"76\" height=\"40\" rx=\"5\" fill=\"{detail}\"/>\
                 <rect x=\"206\" y=\"224\" width=\"76\" height=\"40\" rx=\"5\" fill=\"{detail}\"/>\
                 <rect x=\"300\" y=\"224\" width=\"40\" height=\"40\" rx=\"5\" fill=\"{detail}\" opacity=\".55\"/>\
                 <rect x=\"370\" y=\"220\" width=\"12\" height=\"48\" rx=\"3\" fill=\"{detail}\"/>"
            ),
            Shape::Lamp => format!(
                "<circle cx=\"240\" cy=\"212\" r=\"88\" fill=\"{body}\"/>\
                 <circle cx=\"208\" cy=\"182\" r=\"30\" {gloss}/>\
                 <rect x=\"200\" y=\"288\" width=\"80\" height=\"62\" rx=\"12\" fill=\"{detail}\"/>\
                 <rect x=\"200\" y=\"300\" width=\"80\" height=\"9\" fill=\"{body}\" opacity=\".6\"/>"
            ),
            Shape::Parcel => format!(
                "<rect x=\"118\" y=\"146\" width=\"244\" height=\"202\" rx=\"18\" fill=\"{body}\"/>\
                 <rect x=\"118\" y=\"146\" width=\"244\" height=\"52\" rx=\"18\" fill=\"{detail}\"/>\
                 <rect x=\"138\" y=\"212\" width=\"84\" height=\"26\" rx=\"8\" {gloss}/>\
                 <rect x=\"222\" y=\"198\" width=\"36\" height=\"150\" fill=\"{detail}\" opacity=\".45\"/>"
            ),
        }
    }
}

#[route(GET "/media/demo/{file}")]
pub async fn placeholder(cx: &Cx) -> Result<Svg> {
    let file = param::<File>(cx)?;
    // Only what a slug is made of ever reaches the drawing.
    let reference = file
        .strip_suffix(".svg")
        .filter(|name| timada_core::slug::is_slug(name) && name.len() <= 40)
        .ok_or_not_found()?;
    let hue = reference
        .bytes()
        .fold(0u32, |sum, byte| (sum * 31 + u32::from(byte)) % 360);
    // Desaturated on purpose: a shop's photography is the object, not the
    // colour. The hue only keeps two neighbouring products apart.
    let body = format!("hsl({hue} 12% 72%)");
    let detail = format!("hsl({hue} 14% 46%)");
    let object = Shape::of(reference, hue).draw(&body, &detail);
    let label = reference.to_uppercase();
    // The plate and the caption follow the reader's colour scheme; the object
    // does not, so it reads as the same object in both. An `<img>` still
    // evaluates the media query inside the SVG it points at.
    Ok(Svg(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 480 480\" role=\"img\" aria-label=\"{label}\">\
         <style>.plate{{fill:#f5f5f7}}.caption{{fill:#6e6e73}}\
         @media(prefers-color-scheme:dark){{.plate{{fill:#2a2a2c}}.caption{{fill:#a1a1a6}}}}</style>\
         <rect class=\"plate\" width=\"480\" height=\"480\"/>\
         <ellipse cx=\"240\" cy=\"394\" rx=\"116\" ry=\"13\" fill=\"#000\" opacity=\".08\"/>\
         {object}\
         <text class=\"caption\" x=\"240\" y=\"452\" text-anchor=\"middle\" font-family=\"-apple-system,system-ui,sans-serif\" font-size=\"18\" font-weight=\"500\" letter-spacing=\"1\">{label}</text>\
         </svg>"
    )))
}
