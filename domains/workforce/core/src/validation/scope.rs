use super::{
    common::{Result, require, tags, token, unique},
    context::Context,
    time::date_range,
};
use crate::model::{PersonSelection, Scope, ShiftScope};

impl Context<'_> {
    pub(super) fn people(&self, selection: &PersonSelection, active: bool) -> Result {
        match selection {
            PersonSelection::All {} => Ok(()),
            PersonSelection::Selected { person_ids } => {
                unique(person_ids, active, "personIds")?;
                for id in person_ids {
                    self.person(*id)?;
                }
                Ok(())
            }
            PersonSelection::Filter { all_tags, any_tags } => {
                require(
                    !active || !all_tags.is_empty() || !any_tags.is_empty(),
                    "people",
                    "empty active filter must be explicit all",
                )?;
                tags(all_tags, "allTags")?;
                tags(any_tags, "anyTags")
            }
        }
    }

    pub(super) fn scope(&self, scope: &Scope, active: bool) -> Result {
        self.people(&scope.people, active)?;
        if let Some(ids) = &scope.team_ids {
            unique(ids, active, "teamIds")?;
            for id in ids {
                self.team(*id)?;
            }
        }
        if let Some(ids) = &scope.assignment_type_ids {
            unique(ids, active, "assignmentTypeIds")?;
            for id in ids {
                self.assignment_type(*id)?;
            }
        }
        if let Some(ids) = &scope.location_ids {
            unique(ids, active, "locationIds")?;
            for id in ids {
                self.location(*id)?;
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
                    self.shift(*id)?;
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
                    "scope",
                    "empty active filter must be explicit all",
                )?;
                if let Some(range) = start_date_range {
                    date_range(*range)?;
                }
                if let Some(ids) = assignment_type_ids {
                    unique(ids, active, "assignmentTypeIds")?;
                    for id in ids {
                        self.assignment_type(*id)?;
                    }
                }
                if let Some(ids) = location_ids {
                    unique(ids, active, "locationIds")?;
                    for id in ids {
                        self.location(*id)?;
                    }
                }
                Ok(())
            }
        }
    }
}
