//! Album and track details from the mobile API (public, no cookie needed).

use super::models::{Tralbum, TralbumRef};
use super::{ApiError, Client};

pub async fn tralbum_details(client: &Client, tralbum: TralbumRef) -> Result<Tralbum, ApiError> {
    let band_id = tralbum.band_id.to_string();
    let id = tralbum.id.to_string();
    client
        .get_json(
            "/api/mobile/24/tralbum_details",
            &[
                ("band_id", band_id.as_str()),
                ("tralbum_id", id.as_str()),
                ("tralbum_type", tralbum.kind.as_str()),
            ],
        )
        .await
}
