use activitypub_federation::config::Data;
use base64::{engine::general_purpose::STANDARD_NO_PAD as base64, Engine};
use captcha::Captcha;
use lemmy_api_common::{
  community::BanFromCommunity,
  context::LemmyContext,
  send_activity::{ActivityChannel, SendActivityData},
  utils::check_expire_time,
};
use lemmy_db_schema::{
  source::{
    community::{CommunityActions, CommunityPersonBanForm},
    mod_log::moderator::{ModBanFromCommunity, ModBanFromCommunityForm},
    person::Person,
  },
  traits::{Bannable, Crud, Followable},
};
use lemmy_db_views::structs::LocalUserView;
use lemmy_utils::{
  error::{LemmyErrorExt, LemmyErrorType, LemmyResult},
  utils::slurs::check_slurs,
};
use regex::Regex;
use std::io::Cursor;
use totp_rs::{Secret, TOTP};

pub mod comment;
pub mod community;
pub mod local_user;
pub mod post;
pub mod private_message;
pub mod reports;
pub mod site;
pub mod sitemap;

/// Converts the captcha to a base64 encoded wav audio file
pub(crate) fn captcha_as_wav_base64(captcha: &Captcha) -> LemmyResult<String> {
  let letters = captcha.as_wav();

  // Decode each wav file, concatenate the samples
  let mut concat_samples: Vec<i16> = Vec::new();
  let mut any_header: Option<hound::WavSpec> = None;
  for letter in letters {
    let mut cursor = Cursor::new(letter.unwrap_or_default());
    let reader = hound::WavReader::new(&mut cursor)?;
    any_header = Some(reader.spec());
    let samples16 = reader
      .into_samples::<i16>()
      .collect::<Result<Vec<_>, _>>()
      .with_lemmy_type(LemmyErrorType::CouldntCreateAudioCaptcha)?;
    concat_samples.extend(samples16);
  }

  // Encode the concatenated result as a wav file
  let mut output_buffer = Cursor::new(vec![]);
  if let Some(header) = any_header {
    let mut writer = hound::WavWriter::new(&mut output_buffer, header)
      .with_lemmy_type(LemmyErrorType::CouldntCreateAudioCaptcha)?;
    let mut writer16 = writer.get_i16_writer(concat_samples.len() as u32);
    for sample in concat_samples {
      writer16.write_sample(sample);
    }
    writer16
      .flush()
      .with_lemmy_type(LemmyErrorType::CouldntCreateAudioCaptcha)?;
    writer
      .finalize()
      .with_lemmy_type(LemmyErrorType::CouldntCreateAudioCaptcha)?;

    Ok(base64.encode(output_buffer.into_inner()))
  } else {
    Err(LemmyErrorType::CouldntCreateAudioCaptcha)?
  }
}

/// Check size of report
pub(crate) fn check_report_reason(reason: &str, slur_regex: &Regex) -> LemmyResult<()> {
  check_slurs(reason, slur_regex)?;
  if reason.is_empty() {
    Err(LemmyErrorType::ReportReasonRequired)?
  } else if reason.chars().count() > 1000 {
    Err(LemmyErrorType::ReportTooLong)?
  } else {
    Ok(())
  }
}

pub(crate) fn check_totp_2fa_valid(
  local_user_view: &LocalUserView,
  totp_token: &Option<String>,
  site_name: &str,
) -> LemmyResult<()> {
  // Throw an error if their token is missing
  let token = totp_token
    .as_deref()
    .ok_or(LemmyErrorType::MissingTotpToken)?;
  let secret = local_user_view
    .local_user
    .totp_2fa_secret
    .as_deref()
    .ok_or(LemmyErrorType::MissingTotpSecret)?;

  let totp = build_totp_2fa(site_name, &local_user_view.person.name, secret)?;

  let check_passed = totp.check_current(token)?;
  if !check_passed {
    return Err(LemmyErrorType::IncorrectTotpToken.into());
  }

  Ok(())
}

pub(crate) fn generate_totp_2fa_secret() -> String {
  Secret::generate_secret().to_string()
}

fn build_totp_2fa(hostname: &str, username: &str, secret: &str) -> LemmyResult<TOTP> {
  let sec = Secret::Raw(secret.as_bytes().to_vec());
  let sec_bytes = sec
    .to_bytes()
    .with_lemmy_type(LemmyErrorType::CouldntParseTotpSecret)?;

  TOTP::new(
    totp_rs::Algorithm::SHA1,
    6,
    1,
    30,
    sec_bytes,
    Some(hostname.to_string()),
    username.to_string(),
  )
  .with_lemmy_type(LemmyErrorType::CouldntGenerateTotp)
}

/// Site bans are only federated for local users.
/// This is a problem, because site-banning non-local users will still leave content
/// they've posted to our local communities, on other servers.
///
/// So when doing a site ban for a non-local user, you need to federate/send a
/// community ban for every local community they've participated in.
/// See https://github.com/LemmyNet/lemmy/issues/4118
pub(crate) async fn ban_nonlocal_user_from_local_communities(
  local_user_view: &LocalUserView,
  target: &Person,
  ban: bool,
  reason: &Option<String>,
  remove_or_restore_data: &Option<bool>,
  expires: &Option<i64>,
  context: &Data<LemmyContext>,
) -> LemmyResult<()> {
  // Only run this code for federated users
  if !target.local {
    let ids = Person::list_local_community_ids(&mut context.pool(), target.id).await?;

    for community_id in ids {
      let expires_dt = check_expire_time(*expires)?;

      // Ban / unban them from our local communities
      let community_user_ban_form = CommunityPersonBanForm {
        ban_expires: Some(expires_dt),
        ..CommunityPersonBanForm::new(community_id, target.id)
      };

      if ban {
        // Ignore all errors for these
        CommunityActions::ban(&mut context.pool(), &community_user_ban_form)
          .await
          .ok();

        // Also unsubscribe them from the community, if they are subscribed

        CommunityActions::unfollow(&mut context.pool(), target.id, community_id)
          .await
          .ok();
      } else {
        CommunityActions::unban(&mut context.pool(), &community_user_ban_form)
          .await
          .ok();
      }

      // Mod tables
      let form = ModBanFromCommunityForm {
        mod_person_id: local_user_view.person.id,
        other_person_id: target.id,
        community_id,
        reason: reason.clone(),
        banned: Some(ban),
        expires: expires_dt,
      };

      ModBanFromCommunity::create(&mut context.pool(), &form).await?;

      // Federate the ban from community
      let ban_from_community = BanFromCommunity {
        community_id,
        person_id: target.id,
        ban,
        reason: reason.clone(),
        remove_or_restore_data: *remove_or_restore_data,
        expires: *expires,
      };

      ActivityChannel::submit_activity(
        SendActivityData::BanFromCommunity {
          moderator: local_user_view.person.clone(),
          community_id,
          target: target.clone(),
          data: ban_from_community,
        },
        context,
      )?;
    }
  }

  Ok(())
}

#[cfg(test)]
mod tests {

  use super::*;

  #[test]
  fn test_build_totp() {
    let generated_secret = generate_totp_2fa_secret();
    let totp = build_totp_2fa("lemmy.ml", "my_name", &generated_secret);
    assert!(totp.is_ok());
  }
}
