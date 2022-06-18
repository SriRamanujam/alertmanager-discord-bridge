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
    error, middleware,
    web::{self, Data, Json},
    App, Error, HttpResponse, HttpServer,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, process::exit};

const COLOR_GRAY: i32 = 9807270;
const COLOR_RED: i32 = 15145498;
const COLOR_YELLOW: i32 = 15646767;
const COLOR_BLUE: i32 = 7782616;

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

#[derive(Debug, Serialize, Deserialize)]
struct Author {
    name: String,
    url: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct DiscordEmbedField {
    name: String,
    value: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ReadyzQueryParams {
    verbose: Option<String>,
}

async fn index(
    item: Json<AlertManager>,
    webhook: web::Data<String>,
) -> Result<HttpResponse, Error> {
    log::debug!("Incoming payload: {:?}", &item);

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

    let client = reqwest::Client::new();

    // in general, the difference between a firing alert and a resolved alert is minor, just a couple of small text differences.
    // So we can handle them in a for loop.
    for (status, alerts_by_severity) in grouped_alerts {
        let embeds = alerts_by_severity
            .into_iter()
            .map(|(severity, alerts)| {
                // we are going to turn alerts into DiscordEmbedFields and then make a DiscordEmbed out of the alerts.

                let fields = alerts
                    .iter()
                    .map(|alert| {
                        let name = alert
                            .labels
                            .get("alertname")
                            .cloned()
                            .unwrap_or_else(|| "No-name alert".to_string());
                        let value = alert
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

                        DiscordEmbedField { name, value }
                    })
                    .collect::<Vec<DiscordEmbedField>>();

                let author = Author {
                    name: item
                        .commonLabels
                        .get("prometheus")
                        .cloned()
                        .unwrap_or_default(),
                    url: item.externalURL.clone().replace("///", "//"),
                };

                DiscordEmbed {
                    title: severity.to_uppercase(),
                    description: match severity {
                        "critical" => {
                            "You should take a look at these, like, right now.".to_string()
                        }
                        "warning" => "These are probably issues.".to_string(),
                        "info" => {
                            "These are not bad, but maybe you should take a look?".to_string()
                        }
                        _ => "Unknown severity. Take a look at these".to_string(),
                    },
                    color: match severity {
                        "critical" => COLOR_RED,
                        "warning" => COLOR_YELLOW,
                        "info" => COLOR_BLUE,
                        _ => COLOR_GRAY,
                    },
                    fields,
                    author,
                }
            })
            .collect::<Vec<_>>();

        let discord = Discord {
            content: match status {
                "firing" => "🚨 Your infrastructure would like to inform you about some stuff! 🚨"
                    .to_string(),
                "resolved" => "🎉 These issues have been resolved! 🎉".to_string(),
                _ => format!("Unknown status {}, please advise!", status),
            },
            embeds,
        };

        if discord.embeds.is_empty() {
            log::debug!("No alerts to send, skipping!");
            return Ok(HttpResponse::Ok().finish());
        }

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
                res_string.push_str(&format!("{} {}\n", s, component));
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

    HttpServer::new(|| {
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
            .service(web::resource("/").route(web::post().to(index))) // Main handler route. Send Alertmanager here.
            .service(web::resource("/readyz").route(web::get().to(readyz))) // ready check. Point liveness and readiness checks here.
    })
    .bind(listen_addr)?
    .run()
    .await
}
