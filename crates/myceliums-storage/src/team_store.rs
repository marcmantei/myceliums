use anyhow::{Context, Result};
use arrow_array::{RecordBatch, RecordBatchIterator, StringArray, UInt32Array};
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::{connect, Connection};
use std::path::Path;
use std::sync::Arc;
use tracing::info;

use crate::models::*;
use crate::schema;

/// Escape single quotes in LanceDB filter predicates by doubling them.
/// This prevents SQL injection attacks when values are interpolated into predicates.
fn escape_lance_str(value: &str) -> String {
    value.replace('\'', "''")
}

/// Store for team data. Uses a shared database (not per-repo).
pub struct TeamStore {
    db: Connection,
}

impl TeamStore {
    pub async fn open(db_path: &Path) -> Result<Self> {
        let db = connect(db_path.to_str().unwrap())
            .execute()
            .await
            .context("Failed to open TeamStore")?;
        Ok(Self { db })
    }

    async fn ensure_table(
        &self,
        name: &str,
        schema: arrow_schema::Schema,
    ) -> Result<lancedb::Table> {
        let arc_schema = Arc::new(schema);
        let tables = self.db.table_names().execute().await?;
        if tables.contains(&name.to_string()) {
            Ok(self.db.open_table(name).execute().await?)
        } else {
            let batches = RecordBatchIterator::new(vec![], arc_schema.clone());
            Ok(self.db.create_table(name, batches).execute().await?)
        }
    }

    pub async fn create_team(&self, team: &Team) -> Result<()> {
        let table = self.ensure_table("teams", schema::teams_schema()).await?;

        let schema = Arc::new(schema::teams_schema());
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![team.uid.as_str()])),
                Arc::new(StringArray::from(vec![team.name.as_str()])),
                Arc::new(StringArray::from(vec![team.owner_id.as_str()])),
                Arc::new(StringArray::from(vec![team.created_at.as_str()])),
                Arc::new(UInt32Array::from(vec![team.member_count])),
                Arc::new(StringArray::from(vec![team.repo_ids.as_str()])),
            ],
        )?;

        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
        table.add(batches).execute().await?;
        info!("Created team: {}", team.name);
        Ok(())
    }

    pub async fn get_teams(&self) -> Result<Vec<Team>> {
        let tables = self.db.table_names().execute().await?;
        if !tables.contains(&"teams".to_string()) {
            return Ok(vec![]);
        }
        let table = self.db.open_table("teams").execute().await?;
        let stream = table.query().execute().await?;
        let batches: Vec<RecordBatch> = stream.try_collect().await?;

        let mut teams = Vec::new();
        for batch in &batches {
            let uids = col_str(batch, "uid");
            let names = col_str(batch, "name");
            let owner_ids = col_str(batch, "owner_id");
            let created_ats = col_str(batch, "created_at");
            let member_counts = col_u32(batch, "member_count");
            let repo_ids_col = col_str(batch, "repo_ids");

            for i in 0..batch.num_rows() {
                teams.push(Team {
                    uid: uids.value(i).to_string(),
                    name: names.value(i).to_string(),
                    owner_id: owner_ids.value(i).to_string(),
                    created_at: created_ats.value(i).to_string(),
                    member_count: member_counts.value(i),
                    repo_ids: repo_ids_col.value(i).to_string(),
                });
            }
        }
        Ok(teams)
    }

    pub async fn get_team(&self, team_id: &str) -> Result<Option<Team>> {
        let teams = self.get_teams().await?;
        Ok(teams.into_iter().find(|t| t.uid == team_id))
    }

    pub async fn get_teams_for_user(&self, user_id: &str) -> Result<Vec<Team>> {
        // Get teams where user is owner
        let mut teams: Vec<Team> = self
            .get_teams()
            .await?
            .into_iter()
            .filter(|t| t.owner_id == user_id)
            .collect();

        // Also get teams where user is a member
        let memberships = self.get_memberships_for_user(user_id).await?;
        let all_teams = self.get_teams().await?;
        for membership in &memberships {
            if let Some(team) = all_teams.iter().find(|t| t.uid == membership.team_id) {
                if !teams.iter().any(|t| t.uid == team.uid) {
                    teams.push(team.clone());
                }
            }
        }

        Ok(teams)
    }

    pub async fn add_member(&self, member: &TeamMember) -> Result<()> {
        let table = self
            .ensure_table("team_members", schema::team_members_schema())
            .await?;

        let schema = Arc::new(schema::team_members_schema());
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![member.uid.as_str()])),
                Arc::new(StringArray::from(vec![member.team_id.as_str()])),
                Arc::new(StringArray::from(vec![member.user_id.as_str()])),
                Arc::new(StringArray::from(vec![member.email.as_str()])),
                Arc::new(StringArray::from(vec![member.role.to_string().as_str()])),
                Arc::new(StringArray::from(vec![member.joined_at.as_str()])),
            ],
        )?;

        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
        table.add(batches).execute().await?;
        info!("Added member {} to team {}", member.email, member.team_id);
        Ok(())
    }

    pub async fn get_members(&self, team_id: &str) -> Result<Vec<TeamMember>> {
        let tables = self.db.table_names().execute().await?;
        if !tables.contains(&"team_members".to_string()) {
            return Ok(vec![]);
        }
        let table = self.db.open_table("team_members").execute().await?;
        let escaped_team_id = escape_lance_str(team_id);
        let stream = table
            .query()
            .only_if(format!("team_id = '{}'", escaped_team_id))
            .execute()
            .await?;
        let batches: Vec<RecordBatch> = stream.try_collect().await?;

        let mut members = Vec::new();
        for batch in &batches {
            let uids = col_str(batch, "uid");
            let team_ids = col_str(batch, "team_id");
            let user_ids = col_str(batch, "user_id");
            let emails = col_str(batch, "email");
            let roles = col_str(batch, "role");
            let joined_ats = col_str(batch, "joined_at");

            for i in 0..batch.num_rows() {
                members.push(TeamMember {
                    uid: uids.value(i).to_string(),
                    team_id: team_ids.value(i).to_string(),
                    user_id: user_ids.value(i).to_string(),
                    email: emails.value(i).to_string(),
                    role: roles.value(i).parse().unwrap_or(TeamRole::Viewer),
                    joined_at: joined_ats.value(i).to_string(),
                });
            }
        }
        Ok(members)
    }

    async fn get_memberships_for_user(&self, user_id: &str) -> Result<Vec<TeamMember>> {
        let tables = self.db.table_names().execute().await?;
        if !tables.contains(&"team_members".to_string()) {
            return Ok(vec![]);
        }
        let table = self.db.open_table("team_members").execute().await?;
        let escaped_user_id = escape_lance_str(user_id);
        let stream = table
            .query()
            .only_if(format!("user_id = '{}'", escaped_user_id))
            .execute()
            .await?;
        let batches: Vec<RecordBatch> = stream.try_collect().await?;

        let mut members = Vec::new();
        for batch in &batches {
            let uids = col_str(batch, "uid");
            let team_ids = col_str(batch, "team_id");
            let user_ids = col_str(batch, "user_id");
            let emails = col_str(batch, "email");
            let roles = col_str(batch, "role");
            let joined_ats = col_str(batch, "joined_at");

            for i in 0..batch.num_rows() {
                members.push(TeamMember {
                    uid: uids.value(i).to_string(),
                    team_id: team_ids.value(i).to_string(),
                    user_id: user_ids.value(i).to_string(),
                    email: emails.value(i).to_string(),
                    role: roles.value(i).parse().unwrap_or(TeamRole::Viewer),
                    joined_at: joined_ats.value(i).to_string(),
                });
            }
        }
        Ok(members)
    }
}

fn col_str<'a>(batch: &'a RecordBatch, name: &str) -> &'a StringArray {
    batch
        .column_by_name(name)
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap()
}

fn col_u32<'a>(batch: &'a RecordBatch, name: &str) -> &'a UInt32Array {
    batch
        .column_by_name(name)
        .unwrap()
        .as_any()
        .downcast_ref::<UInt32Array>()
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_lance_str() {
        assert_eq!(escape_lance_str("normal"), "normal");
        assert_eq!(escape_lance_str("O'Brien"), "O''Brien");
        assert_eq!(escape_lance_str("don't"), "don''t");
        assert_eq!(escape_lance_str("it's"), "it''s");
    }
}
