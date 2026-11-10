#!/usr/bin/env python3
"""Shop demo for SQLite: python3 examples/seed/sqlite.py path/to/shop.db"""
import random, sqlite3, sys, os, json, datetime as dt

path = sys.argv[1] if len(sys.argv) > 1 else "shop.db"
if os.path.exists(path):
    os.remove(path)
random.seed(42)
db = sqlite3.connect(path)
db.executescript("""
create table departments (id integer primary key, name text not null unique, city text not null);
create table employees (
  id integer primary key, first_name text not null, last_name text not null, email text not null unique,
  dept_id int not null references departments(id), manager_id int references employees(id),
  hired_on date not null, salary real not null check (salary > 0), active int not null default 1);
create table customers (
  id integer primary key, name text not null, email text, phone text, city text not null,
  segment text not null check (segment in ('retail','wholesale','vip')), created_at text not null);
create table products (
  id integer primary key, sku text not null unique, name text not null, category text not null,
  price real not null, stock int not null default 0, attrs text);
create table orders (
  id integer primary key, customer_id int not null references customers(id), employee_id int references employees(id),
  status text not null default 'new' check (status in ('new','paid','shipped','delivered','cancelled','returned')),
  created_at text not null, shipped_at text, total real not null default 0, note text);
create table order_items (
  order_id int not null references orders(id) on delete cascade, line int not null,
  product_id int not null references products(id), qty int not null check (qty > 0), price real not null,
  primary key (order_id, line)) without rowid;
create table payments (id integer primary key, order_id int not null references orders(id), paid_at text not null, amount real not null, method text not null);
create table order_audit (id integer primary key, order_id int not null, old_status text, new_status text, changed_at text not null default current_timestamp);
""")
firsts = "Анна Иван Мария Алексей Ольга Дмитрий Елена Сергей Наталья Андрей Татьяна Павел Юлия Максим Ксения Артём".split()
lasts = "Иванов Смирнов Кузнецов Попов Васильев Петров Соколов Михайлов Новиков Фёдоров Морозов Волков".split()
cities = ["Москва", "Санкт-Петербург", "Казань", "Новосибирск", "Екатеринбург", "Нижний Новгород", "Самара", "Краснодар"]
names = ["ООО «Ромашка»", "ИП Сидоров", "АО «Вектор»", "ООО «Север»", "Петрова Анна", "Смирнов Илья", "Кузнецова Дарья", "Орлов Глеб"]
deps = [("Продажи", "Москва"), ("Склад", "Подольск"), ("Доставка", "Москва"), ("Бухгалтерия", "Москва"), ("ИТ", "Санкт-Петербург"), ("Маркетинг", "Москва"), ("Поддержка", "Казань"), ("Закупки", "Екатеринбург")]
db.executemany("insert into departments values (?,?,?)", [(i + 1, n, c) for i, (n, c) in enumerate(deps)])
day = lambda base, span: (dt.datetime(*base) + dt.timedelta(seconds=random.randrange(span * 86400))).isoformat(" ")
db.executemany("insert into employees values (?,?,?,?,?,?,?,?,?)", [
    (i, random.choice(firsts), random.choice(lasts), f"emp{i}@shop.example", 1 + i % 8, None if i <= 8 else 1 + i % 8,
     day((2015, 1, 1), 3500)[:10], round(random.uniform(60000, 300000), -2), int(random.random() > 0.07)) for i in range(1, 201)])
def seg(r): return "retail" if r < 0.8 else "wholesale" if r < 0.97 else "vip"
db.executemany("insert into customers values (?,?,?,?,?,?,?)", [
    (i, f"{random.choice(names)} #{i}", f"client{i}@mail.example" if random.random() < 0.9 else None,
     f"+7 9{random.randrange(10**9):09d}" if random.random() < 0.7 else None, random.choice(cities), seg(random.random()),
     day((2019, 1, 1), 2400)) for i in range(1, 5001)])
cats = [("Ноутбуки", ["Ноутбук Lumen 14", "Ноутбук Pro 16", "Ультрабук Air 13"], 45000, 250000),
        ("Телефоны", ["Смартфон Nova", "Смартфон Mini", "Смартфон Max"], 3000, 140000),
        ("Аксессуары", ["Чехол", "Кабель USB-C", "Наушники", "Мышь"], 300, 15000),
        ("Бытовая техника", ["Чайник", "Пылесос", "Кофемашина"], 1500, 90000)]
prods, pid = [], 0
for cat, words, lo, hi in cats:
    for w in words:
        for g in range(25):
            pid += 1
            prods.append((pid, f"SKU-{pid:05d}", f"{w} v{g % 7 + 1}", cat, round(random.uniform(lo, hi)) - 0.01, random.randrange(500),
                          json.dumps({"гарантия_мес": 6 * (1 + g % 4), "страна": ["Китай", "Россия", "Вьетнам"][g % 3]}, ensure_ascii=False)))
db.executemany("insert into products values (?,?,?,?,?,?,?)", prods)
orders, items, pays = [], [], []
for i in range(1, 100001):
    r = random.random()
    st = "new" if r < .05 else "paid" if r < .12 else "shipped" if r < .22 else "delivered" if r < .9 else "cancelled" if r < .96 else "returned"
    created = day((2021, 1, 1), 1770)
    total = 0
    for line in range(1, 2 + i % 5):
        p = prods[(i * 7 + line * 131) % len(prods)]
        q = random.randint(1, 4)
        items.append((i, line, p[0], q, p[4]))
        total += q * p[4]
    shipped = (dt.datetime.fromisoformat(created) + dt.timedelta(days=random.uniform(1, 6))).isoformat(" ") if st in ("shipped", "delivered", "returned") else None
    orders.append((i, random.randint(1, 5000), random.randint(1, 200), st, created, shipped, round(total, 2),
                   random.choice(["Позвонить перед доставкой", "Подарочная упаковка", "Срочно!"]) if random.random() < .08 else None))
    if st not in ("new", "cancelled"):
        pays.append((i, created, round(total, 2), ["карта", "СБП", "наличные", "счёт"][i % 4]))
db.executemany("insert into orders values (?,?,?,?,?,?,?,?)", orders)
db.executemany("insert into order_items values (?,?,?,?,?)", items)
db.executemany("insert into payments (order_id, paid_at, amount, method) values (?,?,?,?)", pays)
db.executescript("""
create index orders_customer on orders(customer_id);
create index orders_created on orders(created_at);
create index order_items_product on order_items(product_id);
create view v_order_totals as
  select o.id, date(o.created_at) day, c.name customer, c.city, o.status, o.total,
         (select count(*) from order_items i where i.order_id = o.id) lines
    from orders o join customers c on c.id = o.customer_id;
create view v_staff as
  select e.id, e.last_name || ' ' || e.first_name full_name, d.name department, m.last_name || ' ' || m.first_name manager, e.salary
    from employees e join departments d on d.id = e.dept_id left join employees m on m.id = e.manager_id where e.active;
create trigger orders_status_audit after update of status on orders when old.status <> new.status
begin insert into order_audit (order_id, old_status, new_status) values (old.id, old.status, new.status); end;
analyze;
""")
db.commit()
print(path, "orders", len(orders), "items", len(items))
