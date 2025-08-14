import { test, expect } from '@playwright/test';

test('test', async ({ page }) => {
  await page.goto('http://localhost:3000/market');
  await page.getByRole('textbox').click();
  await page.getByRole('textbox').fill('mouse 345');
  await page.getByRole('textbox').press('Enter');
  await expect(page.getByRole('main')).toContainText('mouse 345');
});
