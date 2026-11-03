"""Prints 800 order rows as CSV, the way a data script prints a table.

Run it with `uv run python print_orders.py`. scripts/ab-test.sh uses it to see what a model gets
when rtk cuts the output of `uv run` and when dtk summarizes it."""
import random

random.seed(7)
print("order_id,customer,region,amount,status")
for i in range(800):
    print(f"{100000 + i},cust_{random.randint(0, 96):04d},{random.choice(['us-east', 'eu-west', 'ap-south'])},{random.gauss(120, 40):.2f},{random.choice(['ok', 'ok', 'ok', 'retry', 'failed'])}")
