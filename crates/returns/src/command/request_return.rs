use evento::Executor;
use timada_order::{OrderStatus, load_order_details};

use crate::{aggregator::ReturnRequested, error::ReturnError, value_object::ReturnLine};

use super::return_id;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestedLine {
    pub product_id: String,
    pub quantity: u32,
}

#[derive(Debug, Clone)]
pub struct RequestReturn {
    pub order_id: String,
    /// Who asks: someone else's order is reported as not found.
    pub customer_id: String,
    pub lines: Vec<RequestedLine>,
    pub reason: String,
}

impl<E: Executor> super::Command<'_, E> {
    /// Asks to send lines of a shipped order back and returns the new
    /// return's id. The order must be the customer's, shipped, and inside
    /// the policy window; a line can never be returned more often than it was
    /// bought, all of the order's returns taken together.
    pub async fn request_return(&self, cmd: RequestReturn) -> Result<String, ReturnError> {
        if cmd.reason.trim().is_empty() {
            return Err(ReturnError::Required("reason"));
        }
        let order = load_order_details(self.executor, &cmd.order_id)
            .await?
            .filter(|o| o.customer_id == cmd.customer_id)
            .ok_or(ReturnError::OrderNotFound)?;
        let Some(shipped_at) = order
            .shipped_at
            .filter(|_| order.status == OrderStatus::Shipped)
        else {
            return Err(ReturnError::OrderNotShipped);
        };
        if timada_core::time::now_unix_secs()? > self.policy.deadline(shipped_at) {
            return Err(ReturnError::WindowClosed);
        }

        // One entry per product, frozen with the order's name and price.
        let mut lines: Vec<ReturnLine> = Vec::new();
        for requested in cmd.lines.iter().filter(|l| l.quantity > 0) {
            let Some(bought) = order
                .lines
                .iter()
                .find(|l| l.product_id == requested.product_id)
            else {
                return Err(ReturnError::UnknownLine(requested.product_id.clone()));
            };
            match lines.iter_mut().find(|l| l.product_id == bought.product_id) {
                Some(line) => line.quantity = line.quantity.saturating_add(requested.quantity),
                None => lines.push(ReturnLine {
                    product_id: bought.product_id.clone(),
                    name: bought.name.clone(),
                    quantity: requested.quantity,
                    unit_price: bought.unit_price.clone(),
                }),
            }
        }
        if lines.is_empty() {
            return Err(ReturnError::NoLines);
        }

        let bought: Vec<(String, u32)> = lines
            .iter()
            .map(|line| {
                let quantity = order
                    .lines
                    .iter()
                    .filter(|l| l.product_id == line.product_id)
                    .map(|l| l.quantity)
                    .sum();
                (line.product_id.clone(), quantity)
            })
            .collect();
        let rma_number = self.claim(&order.id, &lines, &bought).await?;

        let id = return_id(&rma_number);
        let committed = evento::append(&id)
            .event(&ReturnRequested {
                rma_number: rma_number.clone(),
                order_id: order.id.clone(),
                customer_id: cmd.customer_id,
                lines,
                reason: cmd.reason.trim().to_owned(),
            })
            .commit(self.executor)
            .await;
        if let Err(err) = committed {
            // Nothing was requested after all: give the units back.
            if let Err(release) = self.release_claims(&id).await {
                tracing::warn!(error = %release, %rma_number, "orphan return claims");
            }
            return Err(err.into());
        }
        tracing::info!(return_id = %id, %rma_number, order_id = %order.id, "return requested");
        Ok(id)
    }

    /// In one `BEGIN IMMEDIATE` transaction: checks every line against what
    /// the order's other returns already hold, takes the next RMA number and
    /// records this return's claims under the id derived from it.
    async fn claim(
        &self,
        order_id: &str,
        lines: &[ReturnLine],
        bought: &[(String, u32)],
    ) -> Result<String, ReturnError> {
        let year = timada_core::time::year_of(timada_core::time::now_unix_secs()?);
        let mut conn = self.db.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;

        let claimed: Result<Result<String, ReturnError>, sqlx::Error> = async {
            for line in lines {
                let held: i64 = sqlx::query_scalar(
                    "SELECT COALESCE(SUM(quantity), 0) FROM return_claim
                     WHERE order_id = ? AND product_id = ?",
                )
                .bind(order_id)
                .bind(&line.product_id)
                .fetch_one(&mut *conn)
                .await?;
                let ordered = bought
                    .iter()
                    .find(|(product_id, _)| *product_id == line.product_id)
                    .map_or(0, |(_, quantity)| i64::from(*quantity));
                let returnable = (ordered - held).max(0);
                if i64::from(line.quantity) > returnable {
                    return Ok(Err(ReturnError::QuantityExceeded {
                        product_id: line.product_id.clone(),
                        returnable: returnable as u32,
                    }));
                }
            }
            let number: i64 = sqlx::query_scalar(
                "INSERT INTO return_number (year, order_id) VALUES (?, ?) RETURNING number",
            )
            .bind(year)
            .bind(order_id)
            .fetch_one(&mut *conn)
            .await?;
            let rma_number = format!("R{year}-{number:06}");
            let id = return_id(&rma_number);
            for line in lines {
                sqlx::query(
                    "INSERT INTO return_claim (return_id, order_id, product_id, quantity)
                     VALUES (?, ?, ?, ?)",
                )
                .bind(&id)
                .bind(order_id)
                .bind(&line.product_id)
                .bind(line.quantity)
                .execute(&mut *conn)
                .await?;
            }
            Ok(Ok(rma_number))
        }
        .await;

        match claimed {
            Ok(Ok(rma_number)) => {
                sqlx::query("COMMIT").execute(&mut *conn).await?;
                Ok(rma_number)
            }
            Ok(Err(refused)) => {
                sqlx::query("ROLLBACK").execute(&mut *conn).await?;
                Err(refused)
            }
            Err(err) => {
                if let Err(rollback) = sqlx::query("ROLLBACK").execute(&mut *conn).await {
                    tracing::warn!(error = %rollback, "return claim rollback failed");
                }
                Err(err.into())
            }
        }
    }
}
