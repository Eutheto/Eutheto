use super::{
    common::{Result, prefix, require, tags, token, unique},
    context::Context,
    time::date_range,
};
use crate::model::{PersonSelection, Scope, ShiftScope};
use eutheto_domain_api::DomainPackError;

fn reference_field<T>(result: Result<T>, field: &str) -> Result<T> {
    result.map_err(|error| match error {
        DomainPackError::InvalidPayload { message, .. } => DomainPackError::InvalidPayload {
            path: field.to_owned(),
            message,
        },
        other => other,
    })
}

impl Context<'_> {
    pub(super) fn people(&self, selection: &PersonSelection, active: bool) -> Result {
        match selection {
            PersonSelection::All {} => Ok(()),
            PersonSelection::Selected { person_ids } => {
                unique(person_ids, active, "personIds")?;
                for id in person_ids {
                    reference_field(self.person(*id), "personIds")?;
                }
                Ok(())
            }
            PersonSelection::Filter { all_tags, any_tags } => {
                require(
                    !active || !all_tags.is_empty() || !any_tags.is_empty(),
                    "",
                    "empty active filter must be explicit all",
                )?;
                tags(all_tags, "allTags")?;
                tags(any_tags, "anyTags")
            }
        }
    }

    pub(super) fn scope(&self, scope: &Scope, active: bool) -> Result {
        prefix(self.people(&scope.people, active), "people")?;
        if let Some(ids) = &scope.team_ids {
            unique(ids, active, "teamIds")?;
            for id in ids {
                reference_field(self.team(*id), "teamIds")?;
            }
        }
        if let Some(ids) = &scope.assignment_type_ids {
            unique(ids, active, "assignmentTypeIds")?;
            for id in ids {
                reference_field(self.assignment_type(*id), "assignmentTypeIds")?;
            }
        }
        if let Some(ids) = &scope.location_ids {
            unique(ids, active, "locationIds")?;
            for id in ids {
                reference_field(self.location(*id), "locationIds")?;
            }
        }
        if let Some(categories) = &scope.categories {
            unique(categories, active, "categories")?;
            for category in categories {
                token(category, "categories")?;
            }
        }
        if let Some(weekdays) = &scope.weekdays {
            unique(weekdays, active, "weekdays")?;
        }
        Ok(())
    }

    pub(super) fn shift_scope(&self, scope: &ShiftScope, active: bool) -> Result {
        match scope {
            ShiftScope::All {} => Ok(()),
            ShiftScope::Selected { shift_ids } => {
                unique(shift_ids, active, "shiftIds")?;
                for id in shift_ids {
                    reference_field(self.shift(*id), "shiftIds")?;
                }
                Ok(())
            }
            ShiftScope::Filter {
                assignment_type_ids,
                start_date_range,
                location_ids,
            } => {
                require(
                    !active
                        || assignment_type_ids.is_some()
                        || start_date_range.is_some()
                        || location_ids.is_some(),
                    "",
                    "empty active filter must be explicit all",
                )?;
                if let Some(range) = start_date_range {
                    prefix(date_range(*range), "startDateRange")?;
                }
                if let Some(ids) = assignment_type_ids {
                    unique(ids, active, "assignmentTypeIds")?;
                    for id in ids {
                        reference_field(self.assignment_type(*id), "assignmentTypeIds")?;
                    }
                }
                if let Some(ids) = location_ids {
                    unique(ids, active, "locationIds")?;
                    for id in ids {
                        reference_field(self.location(*id), "locationIds")?;
                    }
                }
                Ok(())
            }
        }
    }
}
