-- Shop demo for PostgreSQL. Run as the owner in database "shop":
--   psql -h localhost -p 54329 -U shop -d shop -f examples/seed/pg.sql
select setseed(0.42);

drop schema if exists sales cascade;
drop schema if exists hr cascade;
create schema hr;
create schema sales;

create table hr.departments (
    id    int primary key,
    name  text not null unique,
    city  text not null
);
comment on table hr.departments is 'Отделы компании';

create table hr.employees (
    id          int primary key,
    first_name  text not null,
    last_name   text not null,
    email       text not null unique,
    dept_id     int not null references hr.departments(id),
    manager_id  int references hr.employees(id),
    hired_on    date not null,
    salary      numeric(10,2) not null check (salary > 0),
    active      boolean not null default true
);
comment on table hr.employees is 'Сотрудники; manager_id — руководитель';

create type sales.order_status as enum ('new', 'paid', 'shipped', 'delivered', 'cancelled', 'returned');

create table sales.customers (
    id          int primary key,
    name        text not null,
    email       text,
    phone       text,
    city        text not null,
    segment     text not null check (segment in ('retail', 'wholesale', 'vip')),
    created_at  timestamptz not null
);
comment on table sales.customers is 'Покупатели';

create table sales.products (
    id        int primary key,
    sku       text not null unique,
    name      text not null,
    category  text not null,
    price     numeric(10,2) not null,
    stock     int not null default 0,
    attrs     jsonb not null default '{}'
);

create table sales.orders (
    id           bigint primary key,
    customer_id  int not null references sales.customers(id),
    employee_id  int references hr.employees(id),
    status       sales.order_status not null default 'new',
    created_at   timestamptz not null,
    shipped_at   timestamptz,
    total        numeric(12,2) not null default 0,
    note         text
);

create table sales.order_items (
    order_id    bigint not null references sales.orders(id) on delete cascade,
    line        int not null,
    product_id  int not null references sales.products(id),
    qty         int not null check (qty > 0),
    price       numeric(10,2) not null,
    primary key (order_id, line)
);

create table sales.payments (
    id        bigint generated always as identity primary key,
    order_id  bigint not null references sales.orders(id),
    paid_at   timestamptz not null,
    amount    numeric(12,2) not null,
    method    text not null
);

create table sales.order_audit (
    id          bigint generated always as identity primary key,
    order_id    bigint not null,
    old_status  sales.order_status,
    new_status  sales.order_status,
    changed_at  timestamptz not null default now(),
    changed_by  text not null default current_user
);

create sequence sales.invoice_no_seq start 100000;

-- Data ------------------------------------------------------------------

insert into hr.departments values
 (1, 'Продажи', 'Москва'), (2, 'Склад', 'Подольск'), (3, 'Доставка', 'Москва'),
 (4, 'Бухгалтерия', 'Москва'), (5, 'ИТ', 'Санкт-Петербург'), (6, 'Маркетинг', 'Москва'),
 (7, 'Поддержка', 'Казань'), (8, 'Закупки', 'Екатеринбург');

with n as (
  select g,
         (array['Анна','Иван','Мария','Алексей','Ольга','Дмитрий','Елена','Сергей','Наталья','Андрей','Татьяна','Павел','Юлия','Максим','Ксения','Артём'])[1 + (random()*15)::int] fn,
         (array['Иванов','Смирнов','Кузнецов','Попов','Васильев','Петров','Соколов','Михайлов','Новиков','Фёдоров','Морозов','Волков','Алексеев','Лебедев','Семёнов'])[1 + (random()*14)::int] ln
  from generate_series(1, 300) g)
insert into hr.employees
select g, fn, ln, 'emp' || g || '@shop.example', 1 + (g % 8),
       case when g <= 8 then null else 1 + (g % 8) end,
       date '2015-01-01' + (random() * 3500)::int,
       round((60000 + random() * 240000)::numeric, -2),
       random() > 0.07
from n;

with n as (
  select g,
         (array['ООО «Ромашка»','ИП Сидоров','АО «Вектор»','ООО «Север»','ООО «Альфа»','Петрова Анна','Смирнов Илья','Кузнецова Дарья','Орлов Глеб','ООО «Техно»','Захарова Вера','ЗАО «Нева»'])[1 + (random()*11)::int] nm,
         (array['Москва','Санкт-Петербург','Казань','Новосибирск','Екатеринбург','Нижний Новгород','Самара','Ростов-на-Дону','Краснодар','Пермь','Воронеж','Уфа'])[1 + (random()*11)::int] city,
         random() r
  from generate_series(1, 20000) g)
insert into sales.customers
select g, nm || ' #' || g,
       case when r < 0.9 then 'client' || g || '@mail.example' end,
       case when r < 0.7 then '+7 9' || lpad((random()*999999999)::bigint::text, 9, '0') end,
       city,
       case when r < 0.8 then 'retail' when r < 0.97 then 'wholesale' else 'vip' end,
       timestamptz '2019-01-01' + random() * interval '2400 days'
from n;

with c(cat, words, lo, hi) as (values
  ('Ноутбуки',  array['Ноутбук Lumen 14','Ноутбук Pro 16','Ультрабук Air 13','Игровой ноутбук X17'], 45000, 250000),
  ('Телефоны',  array['Смартфон Nova','Смартфон Mini','Смартфон Max','Кнопочный телефон'], 3000, 140000),
  ('Аксессуары',array['Чехол','Кабель USB-C','Зарядка 65 Вт','Наушники','Мышь','Клавиатура'], 300, 15000),
  ('Бытовая техника', array['Чайник','Пылесос','Микроволновка','Кофемашина','Утюг'], 1500, 90000),
  ('Мебель',    array['Кресло офисное','Стол письменный','Полка','Шкаф'], 2500, 60000))
insert into sales.products
select row_number() over (), 'SKU-' || lpad((row_number() over ())::text, 5, '0'),
       w || ' ' || (array['белый','чёрный','серый','синий','красный'])[1 + (g % 5)] || ' v' || (g % 7 + 1),
       cat, round((lo + random() * (hi - lo))::numeric, 0) - 0.01,
       (random() * 500)::int,
       jsonb_build_object('гарантия_мес', (array[6,12,24,36])[1 + g % 4], 'вес_кг', round((random()*10)::numeric, 2), 'страна', (array['Китай','Россия','Вьетнам','Корея'])[1 + g % 4])
from c, unnest(words) w, generate_series(1, 40) g;

insert into sales.orders (id, customer_id, employee_id, status, created_at, note)
select g, 1 + (random()*19999)::int, 1 + (random()*299)::int,
       (case when r < 0.05 then 'new' when r < 0.12 then 'paid' when r < 0.22 then 'shipped' when r < 0.9 then 'delivered' when r < 0.96 then 'cancelled' else 'returned' end)::sales.order_status,
       ts,
       case when random() < 0.08 then (array['Позвонить перед доставкой','Подарочная упаковка','Оставить у двери','Срочно!','Счёт на юрлицо'])[1 + (random()*4)::int] end
from (select g, random() r, timestamptz '2021-01-01' + random() * interval '1770 days' ts from generate_series(1, 300000) g) s;

update sales.orders set shipped_at = created_at + (1 + random()*5) * interval '1 day' where status in ('shipped','delivered','returned');

insert into sales.order_items
select o.id, l, p.id, 1 + (random()*3)::int, p.price
from sales.orders o
cross join lateral generate_series(1, 1 + (o.id % 5)::int) l
join sales.products p on p.id = 1 + ((o.id * 7 + l * 131) % (select count(*) from sales.products));

update sales.orders o set total = s.t
from (select order_id, sum(qty * price) t from sales.order_items group by order_id) s
where s.order_id = o.id;

insert into sales.payments (order_id, paid_at, amount, method)
select id, created_at + random() * interval '2 days', total,
       (array['карта','СБП','наличные','счёт'])[1 + (id % 4)::int]
from sales.orders where status not in ('new','cancelled');

create index on sales.orders (customer_id);
create index on sales.orders (created_at);
create index on sales.orders (status) where status in ('new','paid');
create index on sales.order_items (product_id);
create index on sales.payments (order_id);
create index on hr.employees (dept_id);
create index customers_city_idx on sales.customers (city, segment);

-- Objects -----------------------------------------------------------------

create view sales.v_order_totals as
select o.id, o.created_at::date as day, c.name as customer, c.city, o.status, o.total,
       count(i.*) as lines
  from sales.orders o
  join sales.customers c on c.id = o.customer_id
  left join sales.order_items i on i.order_id = o.id
 group by o.id, c.name, c.city;

create view hr.v_staff as
select e.id, e.last_name || ' ' || e.first_name as full_name, d.name as department,
       m.last_name || ' ' || m.first_name as manager, e.salary
  from hr.employees e
  join hr.departments d on d.id = e.dept_id
  left join hr.employees m on m.id = e.manager_id
 where e.active;

create materialized view sales.mv_daily_revenue as
select created_at::date as day, count(*) as orders, sum(total) as revenue
  from sales.orders
 where status not in ('cancelled','returned')
 group by 1;
create unique index on sales.mv_daily_revenue (day);

create function sales.customer_ltv(p_customer int) returns numeric
language sql stable as $$
  select coalesce(sum(total), 0) from sales.orders
   where customer_id = p_customer and status not in ('cancelled','returned')
$$;

create procedure sales.close_stale_orders(p_days int default 30)
language plpgsql as $$
declare
  n int;
begin
  update sales.orders set status = 'cancelled'
   where status = 'new' and created_at < now() - make_interval(days => p_days);
  get diagnostics n = row_count;
  raise notice 'Отменено заказов: %', n;
end $$;

create function sales.audit_status() returns trigger
language plpgsql as $$
begin
  if new.status is distinct from old.status then
    insert into sales.order_audit (order_id, old_status, new_status) values (old.id, old.status, new.status);
  end if;
  return new;
end $$;

create trigger orders_status_audit after update of status on sales.orders
for each row execute function sales.audit_status();

analyze hr.departments, hr.employees, sales.customers, sales.products, sales.orders, sales.order_items, sales.payments;
