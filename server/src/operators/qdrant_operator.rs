use super::search_operator::{assemble_qdrant_filter, SearchResult};
use crate::{
    data::models::{ChunkMetadata, ServerDatasetConfiguration},
    errors::{DefaultError, ServiceError},
    get_env,
    handlers::chunk_handler::ChunkFilter,
};
use itertools::Itertools;
use qdrant_client::{
    client::{QdrantClient, QdrantClientConfig},
    qdrant::{
        group_id::Kind, point_id::PointIdOptions, quantization_config::Quantization,
        with_payload_selector::SelectorOptions, BinaryQuantization, CountPoints, CreateCollection,
        Distance, FieldType, Filter, HnswConfigDiff, PointId, PointStruct, QuantizationConfig,
        RecommendPointGroups, RecommendPoints, SearchPointGroups, SearchPoints, SparseIndexConfig,
        SparseVectorConfig, SparseVectorParams, Value, Vector, VectorParams, VectorParamsMap,
        VectorsConfig, WithPayloadSelector,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::HashMap, str::FromStr};

#[tracing::instrument]
pub async fn get_qdrant_connection(
    qdrant_url: Option<&str>,
    qdrant_api_key: Option<&str>,
) -> Result<QdrantClient, DefaultError> {
    let qdrant_url = qdrant_url.unwrap_or(get_env!(
        "QDRANT_URL",
        "QDRANT_URL should be set if this is called"
    ));
    let qdrant_api_key = qdrant_api_key.unwrap_or(get_env!(
        "QDRANT_API_KEY",
        "QDRANT_API_KEY should be set if this is called"
    ));
    let mut config = QdrantClientConfig::from_url(qdrant_url);
    config.api_key = Some(qdrant_api_key.to_owned());
    QdrantClient::new(Some(config)).map_err(|_err| DefaultError {
        message: "Failed to connect to Qdrant",
    })
}

/// Create Qdrant collection and indexes needed
#[tracing::instrument]
pub async fn create_new_qdrant_collection_query(
    qdrant_url: Option<&str>,
    qdrant_api_key: Option<&str>,
    qdrant_collection: Option<&str>,
    quantize: bool,
) -> Result<(), ServiceError> {
    let qdrant_collection = qdrant_collection
        .unwrap_or(get_env!(
            "QDRANT_COLLECTION",
            "QDRANT_COLLECTION should be set if this is called"
        ))
        .to_string();

    let qdrant_client = get_qdrant_connection(qdrant_url, qdrant_api_key)
        .await
        .map_err(|err| ServiceError::BadRequest(err.message.into()))?;

    // check if collection exists
    let collection = qdrant_client
        .collection_info(qdrant_collection.clone())
        .await;
    if let Ok(collection) = collection {
        if collection.result.is_some() {
            return Err(ServiceError::BadRequest(
                "Collection already exists".to_string(),
            ));
        }
    }

    let mut sparse_vector_config = HashMap::new();
    sparse_vector_config.insert(
        "sparse_vectors".to_string(),
        SparseVectorParams {
            index: Some(SparseIndexConfig {
                on_disk: Some(false),
                ..Default::default()
            }),
        },
    );

    let quantization_config = if quantize {
        Some(QuantizationConfig {
            quantization: Some(Quantization::Binary(BinaryQuantization {
                always_ram: Some(true),
            })),
        })
    } else {
        None
    };

    qdrant_client
        .create_collection(&CreateCollection {
            collection_name: qdrant_collection.clone(),
            vectors_config: Some(VectorsConfig {
                config: Some(qdrant_client::qdrant::vectors_config::Config::ParamsMap(
                    VectorParamsMap {
                        map: HashMap::from([
                            (
                                "384_vectors".to_string(),
                                VectorParams {
                                    size: 384,
                                    distance: Distance::Cosine.into(),
                                    hnsw_config: None,
                                    quantization_config: quantization_config.clone(),
                                    on_disk: None,
                                },
                            ),
                            (
                                "512_vectors".to_string(),
                                VectorParams {
                                    size: 512,
                                    distance: Distance::Cosine.into(),
                                    hnsw_config: None,
                                    quantization_config: None,
                                    on_disk: None,
                                },
                            ),
                            (
                                "768_vectors".to_string(),
                                VectorParams {
                                    size: 768,
                                    distance: Distance::Cosine.into(),
                                    hnsw_config: None,
                                    quantization_config: quantization_config.clone(),
                                    on_disk: None,
                                },
                            ),
                            (
                                "1024_vectors".to_string(),
                                VectorParams {
                                    size: 1024,
                                    distance: Distance::Cosine.into(),
                                    hnsw_config: None,
                                    quantization_config: quantization_config.clone(),
                                    on_disk: None,
                                },
                            ),
                            (
                                "1536_vectors".to_string(),
                                VectorParams {
                                    size: 1536,
                                    distance: Distance::Cosine.into(),
                                    hnsw_config: None,
                                    quantization_config,
                                    on_disk: None,
                                },
                            ),
                        ]),
                    },
                )),
            }),
            hnsw_config: Some(HnswConfigDiff {
                payload_m: Some(16),
                m: Some(0),
                ..Default::default()
            }),
            sparse_vectors_config: Some(SparseVectorConfig {
                map: sparse_vector_config,
            }),
            ..Default::default()
        })
        .await
        .map_err(|err| {
            if err.to_string().contains("already exists") {
                return ServiceError::BadRequest("Collection already exists".into());
            }
            ServiceError::BadRequest("Failed to create Collection".into())
        })?;

    qdrant_client
        .create_field_index(
            qdrant_collection.clone(),
            "link",
            FieldType::Text,
            None,
            None,
        )
        .await
        .map_err(|_| ServiceError::BadRequest("Failed to create index".into()))?;

    qdrant_client
        .create_field_index(
            qdrant_collection.clone(),
            "tag_set",
            FieldType::Text,
            None,
            None,
        )
        .await
        .map_err(|_| ServiceError::BadRequest("Failed to create index".into()))?;

    qdrant_client
        .create_field_index(
            qdrant_collection.clone(),
            "dataset_id",
            FieldType::Keyword,
            None,
            None,
        )
        .await
        .map_err(|_| ServiceError::BadRequest("Failed to create index".into()))?;

    qdrant_client
        .create_field_index(
            qdrant_collection.clone(),
            "metadata",
            FieldType::Keyword,
            None,
            None,
        )
        .await
        .map_err(|_| ServiceError::BadRequest("Failed to create index".into()))?;

    qdrant_client
        .create_field_index(
            qdrant_collection.clone(),
            "time_stamp",
            FieldType::Integer,
            None,
            None,
        )
        .await
        .map_err(|_| ServiceError::BadRequest("Failed to create index".into()))?;

    qdrant_client
        .create_field_index(
            qdrant_collection.clone(),
            "group_ids",
            FieldType::Keyword,
            None,
            None,
        )
        .await
        .map_err(|_| ServiceError::BadRequest("Failed to create index".into()))?;

    Ok(())
}

#[tracing::instrument(skip(embedding_vector))]
pub async fn create_new_qdrant_point_query(
    point_id: uuid::Uuid,
    embedding_vector: Vec<f32>,
    chunk_metadata: ChunkMetadata,
    splade_vector: Vec<(u32, f32)>,
    group_ids: Option<Vec<uuid::Uuid>>,
    config: ServerDatasetConfiguration,
) -> Result<(), actix_web::Error> {
    let qdrant_collection = config.QDRANT_COLLECTION_NAME;

    let qdrant = get_qdrant_connection(Some(&config.QDRANT_URL), Some(&config.QDRANT_API_KEY))
        .await
        .map_err(|err| ServiceError::BadRequest(err.message.into()))?;

    let payload = json!({
        "tag_set": chunk_metadata.tag_set.unwrap_or("".to_string()).split(',').collect_vec(),
        "link": chunk_metadata.link.unwrap_or("".to_string()).split(',').collect_vec(),
        "metadata": chunk_metadata.metadata.unwrap_or_default(),
        "time_stamp": chunk_metadata.time_stamp.unwrap_or_default().timestamp(),
        "dataset_id": chunk_metadata.dataset_id.to_string(),
        "group_ids": group_ids.unwrap_or(vec![])
    })
    .try_into()
    .expect("A json! Value must always be a valid Payload");

    let vector_name = match embedding_vector.len() {
        384 => "384_vectors",
        512 => "512_vectors",
        768 => "768_vectors",
        1024 => "1024_vectors",
        1536 => "1536_vectors",
        _ => return Err(ServiceError::BadRequest("Invalid embedding vector size".into()).into()),
    };

    let vector_payload = HashMap::from([
        (vector_name.to_string(), Vector::from(embedding_vector)),
        ("sparse_vectors".to_string(), Vector::from(splade_vector)),
    ]);

    let point = PointStruct::new(point_id.clone().to_string(), vector_payload, payload);

    qdrant
        .upsert_points_blocking(qdrant_collection, None, vec![point], None)
        .await
        .map_err(|err| {
            sentry::capture_message(&format!("Error {:?}", err), sentry::Level::Error);
            log::error!("Failed inserting chunk to qdrant {:?}", err);
            ServiceError::BadRequest(format!("Failed inserting chunk to qdrant {:?}", err))
        })?;

    Ok(())
}

#[tracing::instrument(skip(updated_vector))]
pub async fn update_qdrant_point_query(
    metadata: Option<ChunkMetadata>,
    point_id: uuid::Uuid,
    updated_vector: Option<Vec<f32>>,
    group_ids: Option<Vec<uuid::Uuid>>,
    dataset_id: uuid::Uuid,
    splade_vector: Vec<(u32, f32)>,
    config: ServerDatasetConfiguration,
) -> Result<(), actix_web::Error> {
    let qdrant_point_id: Vec<PointId> = vec![point_id.to_string().into()];

    let qdrant_collection = config.QDRANT_COLLECTION_NAME;

    let qdrant = get_qdrant_connection(Some(&config.QDRANT_URL), Some(&config.QDRANT_API_KEY))
        .await
        .map_err(|err| ServiceError::BadRequest(err.message.into()))?;

    let current_point_vec = qdrant
        .get_points(
            qdrant_collection.clone(),
            None,
            &qdrant_point_id,
            false.into(),
            true.into(),
            None,
        )
        .await
        .map_err(|_err| ServiceError::BadRequest("Failed to search_points from qdrant".into()))?
        .result;

    let current_point = current_point_vec.first();

    let payload = if let Some(metadata) = metadata.clone() {
        let group_ids = if let Some(group_ids) = group_ids {
            Value::from(
                group_ids
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<String>>(),
            )
        } else if let Some(current_point) = current_point {
            current_point
                .payload
                .get("group_ids")
                .unwrap_or(&Value::from(vec![] as Vec<String>))
                .to_owned()
        } else {
            Value::from(vec![] as Vec<String>)
        };

        json!({
            "tag_set": metadata.tag_set.unwrap_or("".to_string()).split(',').collect_vec(),
            "link": metadata.link.unwrap_or("".to_string()).split(',').collect_vec(),
            "metadata": metadata.metadata.unwrap_or_default(),
            "time_stamp": metadata.time_stamp.unwrap_or_default().timestamp(),
            "dataset_id": dataset_id.to_string(),
            "group_ids": group_ids
        })
    } else if let Some(current_point) = current_point {
        json!({
            "tag_set": current_point.payload.get("tag_set").unwrap_or(&qdrant_client::qdrant::Value::from("")),
            "link": current_point.payload.get("link").unwrap_or(&qdrant_client::qdrant::Value::from("")),
            "metadata": current_point.payload.get("metadata").unwrap_or(&qdrant_client::qdrant::Value::from("")),
            "time_stamp": current_point.payload.get("time_stamp").unwrap_or(&qdrant_client::qdrant::Value::from("")),
            "dataset_id": current_point.payload.get("dataset_id").unwrap_or(&qdrant_client::qdrant::Value::from("")),
            "group_ids": current_point.payload.get("group_ids").unwrap_or(&Value::from(vec![] as Vec<String>))
        })
    } else {
        return Err(ServiceError::BadRequest("No metadata points found".into()).into());
    };

    let points_selector = qdrant_point_id.into();

    if let Some(updated_vector) = updated_vector {
        let vector_name = match updated_vector.len() {
            384 => "384_vectors",
            512 => "512_vectors",
            768 => "768_vectors",
            1024 => "1024_vectors",
            1536 => "1536_vectors",
            _ => {
                return Err(ServiceError::BadRequest("Invalid embedding vector size".into()).into())
            }
        };
        let vector_payload = HashMap::from([
            (vector_name.to_string(), Vector::from(updated_vector)),
            ("sparse_vectors".to_string(), Vector::from(splade_vector)),
        ]);

        let point = PointStruct::new(
            point_id.clone().to_string(),
            vector_payload,
            payload
                .try_into()
                .expect("A json! value must always be a valid Payload"),
        );

        qdrant
            .upsert_points(qdrant_collection, None, vec![point], None)
            .await
            .map_err(|_err| ServiceError::BadRequest("Failed upserting chunk in qdrant".into()))?;

        return Ok(());
    }

    qdrant
        .overwrite_payload(
            qdrant_collection,
            None,
            &points_selector,
            payload
                .try_into()
                .expect("A json! value must always be a valid Payload"),
            None,
            None,
        )
        .await
        .map_err(|_err| {
            ServiceError::BadRequest("Failed updating chunk payload in qdrant".into())
        })?;

    Ok(())
}

#[tracing::instrument]
pub async fn add_bookmark_to_qdrant_query(
    point_id: uuid::Uuid,
    group_id: uuid::Uuid,
    config: ServerDatasetConfiguration,
) -> Result<(), DefaultError> {
    let qdrant_collection = config.QDRANT_COLLECTION_NAME;

    let qdrant =
        get_qdrant_connection(Some(&config.QDRANT_URL), Some(&config.QDRANT_API_KEY)).await?;

    let qdrant_point_id: Vec<PointId> = vec![point_id.to_string().into()];

    let current_point_vec = qdrant
        .get_points(
            qdrant_collection.clone(),
            None,
            &qdrant_point_id,
            false.into(),
            true.into(),
            None,
        )
        .await
        .map_err(|_err| DefaultError {
            message: "Failed to search_points from qdrant",
        })?
        .result;

    let current_point = match current_point_vec.first() {
        Some(point) => point,
        None => {
            return Err(DefaultError {
                message: "Failed getting vec.first chunk from qdrant",
            })
        }
    };

    let group_ids = if current_point.payload.get("group_ids").is_some() {
        let mut group_ids_qdrant = current_point
            .payload
            .get("group_ids")
            .unwrap_or(&Value::from(vec![] as Vec<&str>))
            .iter_list()
            .unwrap()
            .map(|id| {
                id.as_str()
                    .unwrap_or(&"".to_owned())
                    .parse::<uuid::Uuid>()
                    .unwrap_or_default()
            })
            .collect::<Vec<uuid::Uuid>>();
        group_ids_qdrant.append(&mut vec![group_id]);
        group_ids_qdrant
    } else {
        vec![group_id]
    };

    let payload = json!({
        "tag_set": current_point.payload.get("tag_set").unwrap_or(&qdrant_client::qdrant::Value::from("")),
        "link": current_point.payload.get("link").unwrap_or(&qdrant_client::qdrant::Value::from("")),
        "metadata": current_point.payload.get("metadata").unwrap_or(&qdrant_client::qdrant::Value::from("")),
        "time_stamp": current_point.payload.get("time_stamp").unwrap_or(&qdrant_client::qdrant::Value::from("")),
        "dataset_id": current_point.payload.get("dataset_id").unwrap_or(&qdrant_client::qdrant::Value::from("")),
        "group_ids": group_ids
    });

    let points_selector = qdrant_point_id.into();

    qdrant
        .overwrite_payload(
            qdrant_collection,
            None,
            &points_selector,
            payload
                .try_into()
                .expect("A json! value must always be a valid Payload"),
            None,
            None,
        )
        .await
        .map_err(|_err| DefaultError {
            message: "Failed updating chunk payload in qdrant",
        })?;

    Ok(())
}

#[tracing::instrument]
pub async fn remove_bookmark_from_qdrant_query(
    point_id: uuid::Uuid,
    group_id: uuid::Uuid,
    config: ServerDatasetConfiguration,
) -> Result<(), DefaultError> {
    let qdrant_collection = config.QDRANT_COLLECTION_NAME;

    let qdrant =
        get_qdrant_connection(Some(&config.QDRANT_URL), Some(&config.QDRANT_API_KEY)).await?;

    let qdrant_point_id: Vec<PointId> = vec![point_id.to_string().into()];

    let current_point_vec = qdrant
        .get_points(
            qdrant_collection.clone(),
            None,
            &qdrant_point_id,
            false.into(),
            true.into(),
            None,
        )
        .await
        .map_err(|_err| DefaultError {
            message: "Failed to search_points from qdrant",
        })?
        .result;

    let current_point = match current_point_vec.first() {
        Some(point) => point,
        None => {
            return Err(DefaultError {
                message: "Failed getting vec.first chunk from qdrant",
            })
        }
    };

    let group_ids = if current_point.payload.get("group_ids").is_some() {
        let mut group_ids_qdrant = current_point
            .payload
            .get("group_ids")
            .unwrap_or(&Value::from(vec![] as Vec<&str>))
            .iter_list()
            .unwrap()
            .map(|id| {
                id.as_str()
                    .unwrap_or(&"".to_owned())
                    .parse::<uuid::Uuid>()
                    .unwrap_or_default()
            })
            .collect::<Vec<uuid::Uuid>>();
        group_ids_qdrant.retain(|id| id != &group_id);
        group_ids_qdrant
    } else {
        vec![]
    };

    let payload = json!({
        "tag_set": current_point.payload.get("tag_set").unwrap_or(&qdrant_client::qdrant::Value::from("")),
        "link": current_point.payload.get("link").unwrap_or(&qdrant_client::qdrant::Value::from("")),
        "metadata": current_point.payload.get("metadata").unwrap_or(&qdrant_client::qdrant::Value::from("")),
        "time_stamp": current_point.payload.get("time_stamp").unwrap_or(&qdrant_client::qdrant::Value::from("")),
        "dataset_id": current_point.payload.get("dataset_id").unwrap_or(&qdrant_client::qdrant::Value::from("")),
        "group_ids": group_ids
    });

    let points_selector = qdrant_point_id.into();

    qdrant
        .overwrite_payload(
            qdrant_collection,
            None,
            &points_selector,
            payload
                .try_into()
                .expect("A json! value must always be a valid Payload"),
            None,
            None,
        )
        .await
        .map_err(|_err| DefaultError {
            message: "Failed updating chunk payload in qdrant",
        })?;

    Ok(())
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct GroupSearchResults {
    pub group_id: uuid::Uuid,
    pub hits: Vec<SearchResult>,
}

#[derive(Debug)]
pub enum VectorType {
    Sparse(Vec<(u32, f32)>),
    Dense(Vec<f32>),
}

#[tracing::instrument]
pub async fn search_over_groups_query(
    page: u64,
    filter: Filter,
    limit: u32,
    score_threshold: Option<f32>,
    group_size: u32,
    vector: VectorType,
    config: ServerDatasetConfiguration,
) -> Result<Vec<GroupSearchResults>, DefaultError> {
    let qdrant_collection = config.QDRANT_COLLECTION_NAME;

    let qdrant =
        get_qdrant_connection(Some(&config.QDRANT_URL), Some(&config.QDRANT_API_KEY)).await?;

    let vector_name = match vector {
        VectorType::Sparse(_) => "sparse_vectors",
        VectorType::Dense(ref embedding_vector) => match embedding_vector.len() {
            384 => "384_vectors",
            512 => "512_vectors",
            768 => "768_vectors",
            1024 => "1024_vectors",
            1536 => "1536_vectors",
            _ => {
                return Err(DefaultError {
                    message: "Invalid embedding vector size",
                })
            }
        },
    };

    let data = match vector {
        VectorType::Dense(embedding_vector) => {
            qdrant
                .search_groups(&SearchPointGroups {
                    collection_name: qdrant_collection.to_string(),
                    vector: embedding_vector,
                    vector_name: Some(vector_name.to_string()),
                    limit: (limit * page as u32),
                    score_threshold,
                    with_payload: None,
                    filter: Some(filter),
                    group_by: "group_ids".to_string(),
                    group_size,
                    ..Default::default()
                })
                .await
        }

        VectorType::Sparse(sparse_vector) => {
            let sparse_vector: Vector = sparse_vector.into();
            qdrant
                .search_groups(&SearchPointGroups {
                    collection_name: qdrant_collection.to_string(),
                    vector: sparse_vector.data,
                    sparse_indices: sparse_vector.indices,
                    vector_name: Some(vector_name.to_string()),
                    limit: (limit * page as u32),
                    score_threshold,
                    with_payload: None,
                    filter: Some(filter),
                    group_by: "group_ids".to_string(),
                    group_size,
                    ..Default::default()
                })
                .await
        }
    }
    .map_err(|e| {
        log::error!("Failed to search points on Qdrant {:?}", e);
        DefaultError {
            message: "Failed to search points on Qdrant",
        }
    })?;

    let point_ids: Vec<GroupSearchResults> = data
        .result
        .unwrap()
        .groups
        .iter()
        .filter_map(|point| {
            let group_id = match &point.id.clone()?.kind? {
                Kind::StringValue(id) => uuid::Uuid::from_str(id).unwrap_or_default(),
                _ => {
                    return None;
                }
            };

            let hits: Vec<SearchResult> = point
                .hits
                .iter()
                .filter_map(|hit| match hit.id.clone()?.point_id_options? {
                    PointIdOptions::Uuid(id) => Some(SearchResult {
                        score: hit.score,
                        point_id: uuid::Uuid::parse_str(&id).ok()?,
                    }),
                    PointIdOptions::Num(_) => None,
                })
                .collect();

            Some(GroupSearchResults { group_id, hits })
        })
        .collect();

    Ok(point_ids)
}

#[tracing::instrument]
pub async fn search_qdrant_query(
    page: u64,
    filter: Filter,
    limit: u64,
    score_threshold: Option<f32>,
    vector: VectorType,
    config: ServerDatasetConfiguration,
) -> Result<Vec<SearchResult>, DefaultError> {
    let qdrant_collection = config.QDRANT_COLLECTION_NAME;

    let qdrant =
        get_qdrant_connection(Some(&config.QDRANT_URL), Some(&config.QDRANT_API_KEY)).await?;

    let vector_name = match vector {
        VectorType::Sparse(_) => "sparse_vectors",
        VectorType::Dense(ref embedding_vector) => match embedding_vector.len() {
            384 => "384_vectors",
            512 => "512_vectors",
            768 => "768_vectors",
            1024 => "1024_vectors",
            1536 => "1536_vectors",
            _ => {
                return Err(DefaultError {
                    message: "Invalid embedding vector size",
                })
            }
        },
    };

    let data = match vector {
        VectorType::Dense(embedding_vector) => {
            qdrant
                .search_points(&SearchPoints {
                    collection_name: qdrant_collection.to_string(),
                    vector: embedding_vector,
                    vector_name: Some(vector_name.to_string()),
                    limit,
                    score_threshold,
                    offset: Some((page - 1) * 10),
                    with_payload: None,
                    filter: Some(filter),
                    ..Default::default()
                })
                .await
        }

        VectorType::Sparse(sparse_vector) => {
            let sparse_vector: Vector = sparse_vector.into();
            qdrant
                .search_points(&SearchPoints {
                    collection_name: qdrant_collection.to_string(),
                    vector: sparse_vector.data,
                    sparse_indices: sparse_vector.indices,
                    vector_name: Some(vector_name.to_string()),
                    limit,
                    score_threshold,
                    offset: Some((page - 1) * 10),
                    with_payload: None,
                    filter: Some(filter),
                    ..Default::default()
                })
                .await
        }
    }
    .map_err(|e| {
        log::error!("Failed to search points on Qdrant {:?}", e);
        DefaultError {
            message: "Failed to search points on Qdrant",
        }
    })?;

    let point_ids: Vec<SearchResult> = data
        .result
        .iter()
        .filter_map(|point| match point.clone().id?.point_id_options? {
            PointIdOptions::Uuid(id) => Some(SearchResult {
                score: point.score,
                point_id: uuid::Uuid::parse_str(&id).ok()?,
            }),
            PointIdOptions::Num(_) => None,
        })
        .collect();

    Ok(point_ids)
}

#[tracing::instrument]
pub async fn recommend_qdrant_query(
    positive_ids: Vec<uuid::Uuid>,
    negative_ids: Vec<uuid::Uuid>,
    filters: Option<ChunkFilter>,
    limit: u64,
    dataset_id: uuid::Uuid,
    config: ServerDatasetConfiguration,
) -> Result<Vec<uuid::Uuid>, DefaultError> {
    let qdrant_collection = config.QDRANT_COLLECTION_NAME;

    let filter = assemble_qdrant_filter(filters, None, None, dataset_id, None).await?;

    let qdrant =
        get_qdrant_connection(Some(&config.QDRANT_URL), Some(&config.QDRANT_API_KEY)).await?;

    let positive_point_ids: Vec<PointId> = positive_ids
        .iter()
        .map(|id| id.to_string().into())
        .collect();
    let negative_point_ids: Vec<PointId> = negative_ids
        .iter()
        .map(|id| id.to_string().into())
        .collect();

    let vector_name = match config.EMBEDDING_SIZE {
        384 => "384_vectors",
        512 => "512_vectors",
        768 => "768_vectors",
        1024 => "1024_vectors",
        1536 => "1536_vectors",
        _ => {
            return Err(DefaultError {
                message: "Invalid embedding vector size",
            })
        }
    };

    let recommend_points = RecommendPoints {
        collection_name: qdrant_collection,
        positive: positive_point_ids,
        negative: negative_point_ids,
        filter: Some(filter),
        limit,
        with_payload: Some(WithPayloadSelector {
            selector_options: Some(SelectorOptions::Enable(true)),
        }),
        params: None,
        score_threshold: None,
        offset: None,
        using: Some(vector_name.to_string()),
        with_vectors: None,
        lookup_from: None,
        read_consistency: None,
        positive_vectors: vec![],
        negative_vectors: vec![],
        strategy: None,
        timeout: None,
        shard_key_selector: None,
    };

    let recommended_point_ids = qdrant
        .recommend(&recommend_points)
        .await
        .map_err(|err| {
            log::info!("Failed to recommend points from qdrant: {:?}", err);
            DefaultError {
                message: "Failed to recommend points from qdrant. Your are likely providing an invalid point id.",
            }
        })?
        .result
        .into_iter()
        .filter_map(|point| match point.id?.point_id_options? {
            PointIdOptions::Uuid(id) => uuid::Uuid::from_str(&id).ok(),
            PointIdOptions::Num(_) => None,
        })
        .collect::<Vec<uuid::Uuid>>();

    Ok(recommended_point_ids)
}

pub async fn recommend_qdrant_groups_query(
    positive_ids: Vec<uuid::Uuid>,
    negative_ids: Vec<uuid::Uuid>,
    filter: Option<ChunkFilter>,
    limit: u64,
    group_size: u32,
    dataset_id: uuid::Uuid,
    config: ServerDatasetConfiguration,
) -> Result<Vec<GroupSearchResults>, DefaultError> {
    let qdrant_collection = config.QDRANT_COLLECTION_NAME;

    let qdrant =
        get_qdrant_connection(Some(&config.QDRANT_URL), Some(&config.QDRANT_API_KEY)).await?;

    let filters = assemble_qdrant_filter(filter, None, None, dataset_id, None).await?;

    let positive_point_ids: Vec<PointId> = positive_ids
        .iter()
        .map(|id| id.to_string().into())
        .collect();
    let negative_point_ids: Vec<PointId> = negative_ids
        .iter()
        .map(|id| id.to_string().into())
        .collect();

    let vector_name = match config.EMBEDDING_SIZE {
        384 => "384_vectors",
        512 => "512_vectors",
        768 => "768_vectors",
        1024 => "1024_vectors",
        1536 => "1536_vectors",
        _ => {
            return Err(DefaultError {
                message: "Invalid embedding vector size",
            })
        }
    };

    let recommend_points = RecommendPointGroups {
        collection_name: qdrant_collection,
        positive: positive_point_ids,
        negative: negative_point_ids,
        filter: Some(filters),
        limit: limit.try_into().unwrap(),
        with_payload: None,
        params: None,
        score_threshold: None,
        using: Some(vector_name.to_string()),
        with_vectors: None,
        lookup_from: None,
        read_consistency: None,
        positive_vectors: vec![],
        negative_vectors: vec![],
        strategy: None,
        timeout: None,
        shard_key_selector: None,
        group_by: "group_ids".to_string(),
        group_size,
        with_lookup: None,
    };

    let data = qdrant
        .recommend_groups(&recommend_points)
        .await
        .map_err(|err| {
            log::info!("Failed to recommend points from qdrant: {:?}", err);
            DefaultError {
                message: "Failed to recommend points from qdrant. Your are likely providing an invalid point id.",
            }
        })?;
    let recommended_point_ids = data
        .result
        .unwrap()
        .groups
        .iter()
        .filter_map(|point| {
            let group_id = match &point.id.clone()?.kind? {
                Kind::StringValue(id) => uuid::Uuid::from_str(id).unwrap_or_default(),
                _ => {
                    return None;
                }
            };

            let hits: Vec<SearchResult> = point
                .hits
                .iter()
                .filter_map(|hit| match hit.id.clone()?.point_id_options? {
                    PointIdOptions::Uuid(id) => Some(SearchResult {
                        score: hit.score,
                        point_id: uuid::Uuid::parse_str(&id).ok()?,
                    }),
                    PointIdOptions::Num(_) => None,
                })
                .collect();

            Some(GroupSearchResults { group_id, hits })
        })
        .collect();
    Ok(recommended_point_ids)
}

#[tracing::instrument]
pub async fn get_point_count_qdrant_query(
    filters: Filter,
    config: ServerDatasetConfiguration,
) -> Result<u64, DefaultError> {
    let qdrant_collection = config.QDRANT_COLLECTION_NAME;

    let qdrant =
        get_qdrant_connection(Some(&config.QDRANT_URL), Some(&config.QDRANT_API_KEY)).await?;

    let data = qdrant
        .count(&CountPoints {
            collection_name: qdrant_collection,
            filter: Some(filters),
            exact: Some(false),
            read_consistency: None,
            shard_key_selector: None,
        })
        .await
        .map_err(|err| {
            log::info!("Failed to count points from qdrant: {:?}", err);
            DefaultError {
                message: "Failed to count points from qdrant",
            }
        })?;

    Ok(data.result.expect("Failed to get result from qdrant").count)
}
