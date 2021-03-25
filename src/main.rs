#![allow(non_snake_case)]

use actix_web::{
    error, middleware,
    web::{self, Json},
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

async fn index(
    item: Json<AlertManager>,
    webhook: web::Data<String>,
) -> Result<HttpResponse, Error> {
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
                                    .unwrap_or_else(|| String::new())
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
                    url: item.externalURL.clone(),
                };

                DiscordEmbed {
                    title: severity.to_uppercase(),
                    description: match severity {
                        "critical" => {
                            "You should take a look at these, like, right now.".to_string()
                        }
                        "warning" => "These are probably issues.".to_string(),
                        "none" => {
                            "These are not bad, but maybe you should take a look?".to_string()
                        }
                        _ => "Unknown severity. Take a look at these".to_string(),
                    },
                    color: match severity {
                        "critical" => COLOR_RED,
                        "warning" => COLOR_YELLOW,
                        "none" => COLOR_BLUE,
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

        if discord.embeds.len() < 1 {
            log::debug!(
                "No alerts to send, skipping!"
            );
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

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init();

    let listen_addr = std::env::var("LISTEN_ADDRESS").unwrap_or_else(|_| format!("127.0.0.1:9094"));

    HttpServer::new(|| {
        App::new()
            .wrap(middleware::Logger::default())
            .data(match std::env::var("DISCORD_WEBHOOK") {
                Ok(webhook) => webhook,
                Err(_) => {
                    log::error!("Must set DISCORD_WEBHOOK environment variable");
                    exit(1);
                }
            })
            .service(web::resource("/").route(web::post().to(index)))
    })
    .bind(listen_addr)?
    .run()
    .await
}
