use crate::newtypes::{DbUrl, LocalImageId, LocalUserId};
#[cfg(feature = "full")]
use crate::schema::{local_image, remote_image};
use chrono::{DateTime, Utc};
use serde_with::skip_serializing_none;
use std::fmt::Debug;
use typed_builder::TypedBuilder;

#[skip_serializing_none]
#[derive(PartialEq, Eq, Debug, Clone)]
#[cfg_attr(feature = "full", derive(Queryable, Associations, Identifiable))]
#[cfg_attr(feature = "full", diesel(table_name = local_image))]
#[cfg_attr(
  feature = "full",
  diesel(belongs_to(crate::source::local_user::LocalUser))
)]
pub struct LocalImage {
  pub id: LocalImageId,
  pub local_user_id: LocalUserId,
  pub pictrs_alias: String,
  pub pictrs_delete_token: String,
  pub published: DateTime<Utc>,
}

#[derive(Debug, Clone, TypedBuilder)]
#[cfg_attr(feature = "full", derive(Insertable, AsChangeset))]
#[cfg_attr(feature = "full", diesel(table_name = local_image))]
pub struct LocalImageForm {
  pub local_user_id: LocalUserId,
  pub pictrs_alias: String,
  pub pictrs_delete_token: String,
}

#[skip_serializing_none]
#[derive(PartialEq, Eq, Debug, Clone)]
#[cfg_attr(feature = "full", derive(Queryable, Identifiable))]
#[cfg_attr(feature = "full", diesel(table_name = remote_image))]
pub struct RemoteImage {
  pub id: i32,
  pub link: DbUrl,
  pub published: DateTime<Utc>,
}

#[derive(Debug, Clone, TypedBuilder)]
#[cfg_attr(feature = "full", derive(Insertable, AsChangeset))]
#[cfg_attr(feature = "full", diesel(table_name = remote_image))]
pub struct RemoteImageForm {
  pub link: DbUrl,
}
