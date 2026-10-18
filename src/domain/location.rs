const GEO_SCHEME: &str = "geo:";
const PARAMETER_SEPARATOR: char = ';';
const COORDINATE_SEPARATOR: char = ',';
const CRS_PARAMETER: &str = "crs";
const WGS84: &str = "wgs84";
const LATITUDE_LIMIT: f64 = 90.0;
const LONGITUDE_LIMIT: f64 = 180.0;
const MAP_SITE: &str = "https://www.openstreetmap.org/";
const MAP_ZOOM: u8 = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub description: String,
    pub geo_uri: String,
}

impl Location {
    pub fn map_link(&self) -> Option<String> {
        GeoPoint::from_geo_uri(&self.geo_uri).map(GeoPoint::map_link)
    }
}

#[derive(Clone, Copy)]
struct GeoPoint {
    latitude: f64,
    longitude: f64,
}

impl GeoPoint {
    fn from_geo_uri(uri: &str) -> Option<Self> {
        let path = strip_scheme(uri.trim())?;
        let mut parts = path.split(PARAMETER_SEPARATOR);
        let coordinates = parts.next()?;
        if !parts.all(keeps_wgs84) {
            return None;
        }
        let mut values = coordinates.split(COORDINATE_SEPARATOR);
        let latitude = within(number(values.next()?)?, LATITUDE_LIMIT)?;
        let longitude = within(number(values.next()?)?, LONGITUDE_LIMIT)?;
        let altitude_is_valid = values
            .next()
            .is_none_or(|altitude| number(altitude).is_some());
        (altitude_is_valid && values.next().is_none()).then_some(Self {
            latitude,
            longitude,
        })
    }

    fn map_link(self) -> String {
        let Self {
            latitude,
            longitude,
        } = self;
        format!("{MAP_SITE}?mlat={latitude}&mlon={longitude}#map={MAP_ZOOM}/{latitude}/{longitude}")
    }
}

fn strip_scheme(uri: &str) -> Option<&str> {
    let scheme = uri.get(..GEO_SCHEME.len())?;
    scheme
        .eq_ignore_ascii_case(GEO_SCHEME)
        .then(|| uri.get(GEO_SCHEME.len()..))
        .flatten()
}

fn keeps_wgs84(parameter: &str) -> bool {
    let (name, label) = parameter.split_once('=').unwrap_or((parameter, ""));
    !name.eq_ignore_ascii_case(CRS_PARAMETER) || label.eq_ignore_ascii_case(WGS84)
}

fn number(text: &str) -> Option<f64> {
    let unsigned = text.strip_prefix('-').unwrap_or(text);
    let (whole, fraction) = match unsigned.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (unsigned, None),
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    if !digits(whole) || fraction.is_some_and(|fraction| !digits(fraction)) {
        return None;
    }
    text.parse().ok()
}

fn within(value: f64, limit: f64) -> Option<f64> {
    (value.abs() <= limit).then_some(value)
}
