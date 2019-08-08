/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

use crate::api::places_api::PlacesApi;
use crate::bookmark_sync::store::BookmarksStore;
use crate::error::*;
use crate::import::common::attached_database;
use rusqlite::Connection;
use url::Url;

pub fn import(places_api: &PlacesApi, path: impl AsRef<std::path::Path>) -> Result<()> {
    let url = crate::util::ensure_url_path(path)?;
    do_import(places_api, url)
}

fn do_import(places_api: &PlacesApi, android_db_file_url: Url) -> Result<()> {
    let conn = places_api.open_sync_connection()?;

    let scope = conn.begin_interrupt_scope();

    define_sql_functions(&conn)?;

    // Not sure why, but apparently beginning a transaction sometimes
    // fails if we open the DB as read-only. Hopefully we don't
    // unintentionally write to it anywhere...
    // android_db_file_url.query_pairs_mut().append_pair("mode", "ro");

    log::trace!("Attaching database {}", android_db_file_url);
    let auto_detach = attached_database(&conn, &android_db_file_url, "fennec")?;

    let tx = conn.begin_transaction()?;

    log::debug!("Populating missing entries in moz_places");
    conn.execute_batch(&FILL_MOZ_PLACES)?;
    scope.err_if_interrupted()?;

    log::debug!("Inserting the history visits");
    conn.execute_batch(&INSERT_HISTORY_VISITS)?;
    scope.err_if_interrupted()?;

    log::debug!("Committing...");
    tx.commit()?;

    // Note: update_frecencies manages its own transaction, which is fine,
    // since nothing that bad will happen if it is aborted.
    log::debug!("Updating frecencies");
    let store = BookmarksStore::new(&conn, &scope);
    store.update_frecencies()?;

    log::info!("Successfully imported history visits!");

    auto_detach.execute_now()?;

    Ok(())
}

lazy_static::lazy_static! {
    // Insert any missing entries into moz_places that we'll need for this.
    static ref FILL_MOZ_PLACES: &'static str =
        "INSERT OR IGNORE INTO main.moz_places(guid, url, url_hash, title, frecency, sync_change_counter)
            SELECT
                IFNULL(
                    (SELECT p.guid FROM main.moz_places p WHERE p.url_hash = hash(h.url) AND p.url = h.url),
                    generate_guid()
                ),
                h.url,
                hash(h.url),
                h.title,
                -1,
                1
            FROM fennec.history h
            WHERE is_valid_url(h.url)"
    ;

    // Insert history visits
    static ref INSERT_HISTORY_VISITS: &'static str =
        "INSERT OR IGNORE INTO main.moz_historyvisits(from_visit, place_id, visit_date, visit_type, is_local)
            SELECT
                NULL, -- Fenec does not store enough information to rebuild redirect chains.
                (SELECT p.id FROM main.moz_places p WHERE p.url_hash = hash(h.url) AND p.url = h.url),
                sanitize_timestamp(v.date),
                v.visit_type, -- Fennec stores visit types maps 1:1 to ours.
                v.is_local
            FROM fennec.visits v
            LEFT JOIN fennec.history h on v.history_guid = h.guid
            WHERE is_valid_url(h.url)"
    ;
}

pub(super) fn define_sql_functions(c: &Connection) -> Result<()> {
    c.create_scalar_function(
        "is_valid_url",
        1,
        true,
        crate::import::common::sql_fns::is_valid_url,
    )?;
    c.create_scalar_function(
        "sanitize_timestamp",
        1,
        true,
        crate::import::common::sql_fns::sanitize_timestamp,
    )?;
    c.create_scalar_function("hash", -1, true, crate::db::db::sql_fns::hash)?;
    c.create_scalar_function(
        "generate_guid",
        0,
        false,
        crate::db::db::sql_fns::generate_guid,
    )?;
    Ok(())
}
