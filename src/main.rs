/*
This file is part of Alertmanager to Discord Bridge (https://github.com/SriRamanujam/alertmanager-discord-bridge)
Copyright (C) 2021 Sri Ramanujam

This program is free software; you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation; either version 2 of the License, or
(at your option) any later version.

This program is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
GNU General Public License for more details.

You should have received a copy of the GNU General Public License along
with this program; if not, write to the Free Software Foundation, Inc.,
51 Franklin Street, Fifth Floor, Boston, MA 02110-1301 USA
*/

#![allow(non_snake_case)]

use actix_web::{
    App, Error, HttpResponse, HttpServer, error, middleware,
    web::{self, Data, Json},
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, process::exit};

const COLOR_GRAY: i32 = 9807270;
const COLOR_RED: i32 = 15145498;
const COLOR_YELLOW: i32 = 15646767;
const COLOR_BLUE: i32 = 7782616;
const COLOR_GREEN: i32 = 5793266;

// Discord hard limits
const MAX_EMBED_FIELDS: usize = 25;
const MAX_EMBEDS_PER_MESSAGE: usize = 10;
const MAX_FIELD_NAME: usize = 256;
const MAX_FIELD_VALUE: usize = 1024;

#[derive(Debug, Serialize, Deserialize)]
struct AlertManager {
    version: String,
    groupKey: String,
    status: String, // TODO: this can be changed to an enum Resolved/Firing
    receiver: String,
    commonLabels: HashMap<String, String>,
    commonAnnotations: HashMap<String, String>,
    externalURL: String, // backlink to the AlertManager in question
    alerts: Vec<AlertManagerAlert>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AlertManagerAlert {
    status: String, // TODO: change to an enum Resolved/Firing
    labels: HashMap<String, String>,
    annotations: HashMap<String, String>,
    startsAt: String, // TODO: this can be parsed out with chrono
    endsAt: String,   // TODO: this can be parsed out with chrono
    generatorURL: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Discord {
    content: String,
    embeds: Vec<DiscordEmbed>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DiscordEmbed {
    title: String,
    description: String,
    color: i32,
    fields: Vec<DiscordEmbedField>,
    author: Author,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Author {
    name: String,
    url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DiscordEmbedField {
    name: String,
    value: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ReadyzQueryParams {
    verbose: Option<String>,
}

/// Deterministic field order so the same alert set renders identically every send.
fn sort_alerts<'a>(alerts: &[&'a AlertManagerAlert]) -> Vec<&'a AlertManagerAlert> {
    let mut sorted = alerts.to_vec();
    sorted.sort_by(|a, b| {
        let name_a = a.labels.get("alertname").cloned().unwrap_or_default();
        let name_b = b.labels.get("alertname").cloned().unwrap_or_default();
        let inst_a = ["instance", "namespace", "pod", "job"]
            .iter()
            .find_map(|k| a.labels.get(*k).cloned())
            .unwrap_or_default();
        let inst_b = ["instance", "namespace", "pod", "job"]
            .iter()
            .find_map(|k| b.labels.get(*k).cloned())
            .unwrap_or_default();
        (name_a, inst_a).cmp(&(name_b, inst_b))
    });
    sorted
}

/// Split fields into Discord-sized embeds (25 fields max each).
fn chunk_fields(fields: Vec<DiscordEmbedField>) -> Vec<Vec<DiscordEmbedField>> {
    fields
        .chunks(MAX_EMBED_FIELDS)
        .map(|c| c.to_vec())
        .collect()
}

/// Discord hard-caps field names and values; past that the whole POST fails.
fn truncate_field(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

/// Display priority: critical, warning, info, then the rest alphabetically.
fn severity_rank(severity: &str) -> u8 {
    match severity {
        "critical" => 0,
        "warning" => 1,
        "info" => 2,
        _ => 3,
    }
}

/// Render startsAt for a human scrolling the channel; raw string if unparseable.
fn format_start_time(starts_at: &str) -> String {
    match DateTime::parse_from_rfc3339(starts_at) {
        Ok(t) => {
            let t = t.with_timezone(&Utc);
            let age = humanize(Utc::now().signed_duration_since(t));
            format!("🕒 fired {} ({age} ago)", t.format("%Y-%m-%d %H:%M UTC"))
        }
        Err(_) => format!("🕒 started {starts_at}"),
    }
}

fn humanize(d: Duration) -> String {
    let s = d.num_seconds().unsigned_abs();
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86400)
    }
}

async fn index(
    item: Json<AlertManager>,
    webhook: web::Data<String>,
    client: web::Data<reqwest::Client>,
) -> Result<HttpResponse, Error> {
    log::debug!("Incoming payload: {:?}", item);

    // run through the incoming alerts and group them by status and severity
    // HashMap<"status", HashMap<"severity", Vec<AlertManagerAlert>>>
    let mut grouped_alerts = HashMap::<&str, HashMap<&str, Vec<&AlertManagerAlert>>>::new();

    for alert in &item.alerts {
        let alerts_by_severity = grouped_alerts.entry(&alert.status).or_default();

        let severity = match alert.labels.get("severity") {
            Some(s) => s.as_str(),
            None => "none", // if you didn't want to give me a severity, i'm going to assume it's not a problem
        };

        // don't alert on "none" severity alerts. They don't matter.
        if severity == "none" {
            continue;
        }

        alerts_by_severity.entry(severity).or_default().push(alert);
    }

    // in general, the difference between a firing alert and a resolved alert is minor, just a couple of small text differences.
    // So we can handle them in a for loop.
    for (status, alerts_by_severity) in grouped_alerts {
        let content = match status {
            "firing" => {
                "🚨 Your infrastructure would like to inform you about some stuff! 🚨".to_string()
            }
            "resolved" => "🎉 These issues have been resolved! 🎉".to_string(),
            _ => format!("Unknown status {status}, please advise!"),
        };

        let author = Author {
            name: item
                .commonLabels
                .get("prometheus")
                .cloned()
                .unwrap_or_default(),
            url: item.externalURL.clone().replace("///", "//"),
        };

        // deterministic severity order: critical, warning, info, then the rest
        let mut severities: Vec<(String, Vec<&AlertManagerAlert>)> = alerts_by_severity
            .into_iter()
            .map(|(severity, alerts)| (severity.to_string(), alerts))
            .collect();
        severities.sort_by(|a, b| {
            severity_rank(&a.0)
                .cmp(&severity_rank(&b.0))
                .then_with(|| a.0.cmp(&b.0))
        });

        // one embed per 25-field chunk, up to 10 embeds per message
        let mut messages: Vec<Vec<DiscordEmbed>> = Vec::new();
        for (severity, alerts) in severities {
            let fields = sort_alerts(&alerts)
                .iter()
                .map(|alert| {
                    let alertname = alert
                        .labels
                        .get("alertname")
                        .cloned()
                        .unwrap_or_else(|| "No-name alert".to_string());
                    let name = if alert.generatorURL.is_empty() {
                        alertname
                    } else {
                        // link each alert straight to its generator query
                        format!("[{alertname}]({})", alert.generatorURL)
                    };
                    let body = alert
                        .annotations
                        .get("description")
                        .cloned()
                        .unwrap_or_else(|| {
                            alert
                                .annotations
                                .get("message")
                                .cloned()
                                .unwrap_or_default()
                        });
                    let value = format!("{body}\n{}", format_start_time(&alert.startsAt));

                    DiscordEmbedField {
                        name: truncate_field(&name, MAX_FIELD_NAME),
                        value: truncate_field(&value, MAX_FIELD_VALUE),
                    }
                })
                .collect::<Vec<DiscordEmbedField>>();

            for chunk in chunk_fields(fields) {
                let embed = DiscordEmbed {
                    title: severity.to_uppercase(),
                    description: match severity.as_str() {
                        "critical" => {
                            "You should take a look at these, like, right now.".to_string()
                        }
                        "warning" => "These are probably issues.".to_string(),
                        "info" => {
                            "These are not bad, but maybe you should take a look?".to_string()
                        }
                        _ => "Unknown severity. Take a look at these".to_string(),
                    },
                    color: if status == "resolved" {
                        COLOR_GREEN
                    } else {
                        match severity.as_str() {
                            "critical" => COLOR_RED,
                            "warning" => COLOR_YELLOW,
                            "info" => COLOR_BLUE,
                            _ => COLOR_GRAY,
                        }
                    },
                    fields: chunk,
                    author: author.clone(),
                };
                if messages
                    .last()
                    .map_or(true, |m| m.len() >= MAX_EMBEDS_PER_MESSAGE)
                {
                    messages.push(Vec::new());
                }
                messages.last_mut().unwrap().push(embed);
            }
        }

        let embed_count = messages.iter().map(|m| m.len()).sum::<usize>();
        if embed_count == 0 {
            // `continue`, not `return`: other status groups in this payload still need to be sent
            log::debug!("No alerts to send for {status}, skipping!");
            continue;
        }

        for (i, embeds) in messages.into_iter().enumerate() {
            let discord = Discord {
                content: if i == 0 {
                    content.clone()
                } else {
                    String::new()
                },
                embeds,
            };

            log::debug!(
                "Sending discord payload to webhook: {:?}",
                serde_json::to_string(&discord)
            );

            match client.post(webhook.get_ref()).json(&discord).send().await {
                Err(e) => {
                    log::error!("Could not send to Discord: {}", e);
                    return Err(error::ErrorInternalServerError("Could not send to Discord"));
                }
                Ok(res) => {
                    if let Err(e) = res.error_for_status() {
                        log::error!("Discord API returned error: {}", e);
                        return Err(error::ErrorInternalServerError(
                            "Discord API rejected payload",
                        ));
                    }
                }
            }
        }
    }

    log::info!("Dispatched alerts to Discord");

    Ok(HttpResponse::Ok().finish())
}

/// Tests all necessary upstream components to make sure that the service is ready to accept messages.
async fn readyz(
    query: web::Query<ReadyzQueryParams>,
    webhook: web::Data<String>,
) -> Result<HttpResponse, Error> {
    let mut component_statuses = HashMap::new();

    // test connectivity to Discord.
    let discord_success = {
        let test_req = reqwest::get(webhook.get_ref()).await;

        // set the value of discord_success based on the response code of the call to Discord.
        match test_req {
            Ok(res) => {
                if res.status() == reqwest::StatusCode::OK {
                    true
                } else {
                    match res.text().await {
                        Ok(s) => log::warn!("Error talking to Discord: {}", s),
                        Err(_) => log::warn!("Error talking to Discord"),
                    };
                    false
                }
            }
            Err(e) => {
                log::warn!("Discord not reachable: {}", e);
                false
            }
        }
    };
    component_statuses.insert("Discord", discord_success);

    // generate response. If "?verbose" is passed as a query parameter, generate a verbose string.
    if query.0.verbose.is_some() {
        // generate verbose response and respond with 200 or 503
        let mut res_string = String::new();

        let overall_success = component_statuses
            .iter()
            .fold(true, |success, (component, up)| {
                let s = if *up { "[+]" } else { "[-]" };
                res_string.push_str(s);
                res_string.push_str(component);
                res_string.push('\n');
                success & *up
            });

        if overall_success {
            Ok(HttpResponse::Ok().body(res_string))
        } else {
            Ok(HttpResponse::ServiceUnavailable().body(res_string))
        }
    } else {
        // generate 204 or 503
        let overall_success = component_statuses
            .values()
            .fold(true, |success, up| success & *up);

        if overall_success {
            Ok(HttpResponse::NoContent().finish())
        } else {
            Ok(HttpResponse::ServiceUnavailable().finish())
        }
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init();

    let listen_addr =
        std::env::var("LISTEN_ADDRESS").unwrap_or_else(|_| "127.0.0.1:9094".to_string());

    HttpServer::new(move || {
        App::new()
            .wrap(middleware::Logger::default())
            .app_data(Data::new(match std::env::var("DISCORD_WEBHOOK") {
                Ok(webhook) => {
                    if !webhook.is_empty() {
                        webhook
                    } else {
                        log::error!("Must set DISCORD_WEBHOOK environment variable");
                        exit(1);
                    }
                }
                Err(_) => {
                    log::error!("Must set DISCORD_WEBHOOK environment variable");
                    exit(1);
                }
            }))
            .app_data(Data::new(reqwest::Client::new()))
            .service(web::resource("/").route(web::post().to(index))) // Main handler route. Send Alertmanager here.
            .service(web::resource("/readyz").route(web::get().to(readyz))) // ready check. Point liveness and readiness checks here.
    })
    .bind(listen_addr)?
    .run()
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(i: usize) -> DiscordEmbedField {
        DiscordEmbedField {
            name: format!("n{i}"),
            value: format!("v{i}"),
        }
    }

    fn alert(name: &str, instance: Option<&str>) -> AlertManagerAlert {
        let mut labels = HashMap::new();
        labels.insert("alertname".to_string(), name.to_string());
        if let Some(i) = instance {
            labels.insert("instance".to_string(), i.to_string());
        }
        AlertManagerAlert {
            status: "firing".into(),
            labels,
            annotations: HashMap::new(),
            startsAt: "2021-01-01T00:00:00Z".into(),
            endsAt: "0001-01-01T00:00:00Z".into(),
            generatorURL: String::new(),
        }
    }

    #[test]
    fn chunk_fields_respects_embed_limit() {
        let chunks = chunk_fields((0..25).map(field).collect());
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 25);

        let chunks = chunk_fields((0..26).map(field).collect());
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 25);
        assert_eq!(chunks[1].len(), 1);
    }

    fn sort_key(a: &AlertManagerAlert) -> (String, String) {
        let name = a.labels.get("alertname").cloned().unwrap_or_default();
        let inst = ["instance", "namespace", "pod", "job"]
            .iter()
            .find_map(|k| a.labels.get(*k).cloned())
            .unwrap_or_default();
        (name, inst)
    }

    #[test]
    fn sort_alerts_orders_by_name_then_instance() {
        let alerts = vec![
            alert("B", Some("host2")),
            alert("A", Some("host9")),
            alert("B", Some("host1")),
        ];
        let refs: Vec<_> = alerts.iter().collect();
        let sorted = sort_alerts(&refs);
        assert_eq!(
            sorted.iter().map(|a| sort_key(a)).collect::<Vec<_>>(),
            vec![
                ("A".to_string(), "host9".to_string()),
                ("B".to_string(), "host1".to_string()),
                ("B".to_string(), "host2".to_string()),
            ]
        );
        // no instance label: falls back to namespace for ordering (ns-b before ns-a flips)
        let ns_alerts = vec![
            {
                let mut a = alert("X", None);
                a.labels.insert("namespace".to_string(), "ns-b".to_string());
                a
            },
            {
                let mut a = alert("X", None);
                a.labels.insert("namespace".to_string(), "ns-a".to_string());
                a
            },
        ];
        let refs2: Vec<&AlertManagerAlert> = ns_alerts.iter().collect();
        let sorted2 = sort_alerts(&refs2);
        assert_eq!(
            sorted2
                .iter()
                .map(|a| a.labels.get("namespace").cloned())
                .collect::<Vec<_>>(),
            vec![Some("ns-a".to_string()), Some("ns-b".to_string())]
        );
    }

    #[test]
    fn format_start_time_parses_and_falls_back() {
        assert!(format_start_time("2021-01-01T00:00:00Z").contains("2021-01-01 00:00 UTC"));
        assert!(format_start_time("not a time").contains("not a time"));
    }
}
