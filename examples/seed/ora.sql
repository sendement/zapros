-- Shop demo for Oracle (23ai / Free). Run as SHOP:
--   sqlplus shop/shop@//localhost:11521/FREEPDB1 @examples/seed/ora.sql
whenever sqlerror exit failure
set define off feedback off
exec dbms_random.seed(42)

create table departments (
    id    number(4) primary key,
    name  varchar2(100) not null unique,
    city  varchar2(100) not null
);
comment on table departments is 'Отделы компании';

create table employees (
    id          number(6) primary key,
    first_name  varchar2(50) not null,
    last_name   varchar2(50) not null,
    email       varchar2(100) not null unique,
    dept_id     number(4) not null references departments(id),
    manager_id  number(6) references employees(id),
    hired_on    date not null,
    salary      number(10,2) not null check (salary > 0),
    active      char(1) default 'Y' not null check (active in ('Y','N'))
);
comment on table employees is 'Сотрудники; manager_id — руководитель';

create table customers (
    id          number(8) primary key,
    name        varchar2(200) not null,
    email       varchar2(200),
    phone       varchar2(20),
    city        varchar2(100) not null,
    segment     varchar2(20) not null check (segment in ('retail','wholesale','vip')),
    created_at  timestamp not null
);
comment on table customers is 'Покупатели';

create table products (
    id        number(6) primary key,
    sku       varchar2(20) not null unique,
    name      varchar2(200) not null,
    category  varchar2(100) not null,
    price     number(10,2) not null,
    stock     number(6) default 0 not null,
    attrs     varchar2(4000) check (attrs is json)
);

create table orders (
    id           number(10) primary key,
    customer_id  number(8) not null references customers(id),
    employee_id  number(6) references employees(id),
    status       varchar2(20) default 'new' not null
                 check (status in ('new','paid','shipped','delivered','cancelled','returned')),
    created_at   timestamp not null,
    shipped_at   timestamp,
    total        number(12,2) default 0 not null,
    note         varchar2(400)
);

create table order_items (
    order_id    number(10) not null references orders(id) on delete cascade,
    line        number(3) not null,
    product_id  number(6) not null references products(id),
    qty         number(4) not null check (qty > 0),
    price       number(10,2) not null,
    constraint order_items_pk primary key (order_id, line)
);

create table payments (
    id        number generated always as identity primary key,
    order_id  number(10) not null references orders(id),
    paid_at   timestamp not null,
    amount    number(12,2) not null,
    method    varchar2(20) not null
);

create table order_audit (
    id          number generated always as identity primary key,
    order_id    number(10) not null,
    old_status  varchar2(20),
    new_status  varchar2(20),
    changed_at  timestamp default systimestamp not null,
    changed_by  varchar2(128) default user not null
);

create sequence invoice_no_seq start with 100000;

-- Data ------------------------------------------------------------------

insert into departments
select level,
       regexp_substr('Продажи,Склад,Доставка,Бухгалтерия,ИТ,Маркетинг,Поддержка,Закупки', '[^,]+', 1, level),
       regexp_substr('Москва,Подольск,Москва,Москва,Санкт-Петербург,Москва,Казань,Екатеринбург', '[^,]+', 1, level)
from dual connect by level <= 8;

insert into employees
select level,
       regexp_substr('Анна,Иван,Мария,Алексей,Ольга,Дмитрий,Елена,Сергей,Наталья,Андрей,Татьяна,Павел,Юлия,Максим,Ксения,Артём', '[^,]+', 1, 1 + trunc(dbms_random.value(0, 16))),
       regexp_substr('Иванов,Смирнов,Кузнецов,Попов,Васильев,Петров,Соколов,Михайлов,Новиков,Фёдоров,Морозов,Волков,Алексеев,Лебедев,Семёнов', '[^,]+', 1, 1 + trunc(dbms_random.value(0, 15))),
       'emp' || level || '@shop.example',
       1 + mod(level, 8),
       case when level <= 8 then null else 1 + mod(level, 8) end,
       date '2015-01-01' + trunc(dbms_random.value(0, 3500)),
       round(dbms_random.value(60000, 300000), -2),
       case when dbms_random.value < 0.07 then 'N' else 'Y' end
from dual connect by level <= 300;

insert into customers
select g,
       regexp_substr('ООО «Ромашка»,ИП Сидоров,АО «Вектор»,ООО «Север»,ООО «Альфа»,Петрова Анна,Смирнов Илья,Кузнецова Дарья,Орлов Глеб,ООО «Техно»,Захарова Вера,ЗАО «Нева»', '[^,]+', 1, 1 + trunc(dbms_random.value(0, 12))) || ' #' || g,
       case when r < 0.9 then 'client' || g || '@mail.example' end,
       case when r < 0.7 then '+7 9' || lpad(trunc(dbms_random.value(0, 999999999)), 9, '0') end,
       regexp_substr('Москва,Санкт-Петербург,Казань,Новосибирск,Екатеринбург,Нижний Новгород,Самара,Ростов-на-Дону,Краснодар,Пермь,Воронеж,Уфа', '[^,]+', 1, 1 + trunc(dbms_random.value(0, 12))),
       case when r < 0.8 then 'retail' when r < 0.97 then 'wholesale' else 'vip' end,
       timestamp '2019-01-01 00:00:00' + numtodsinterval(dbms_random.value(0, 2400), 'DAY')
from (select level g, dbms_random.value r from dual connect by level <= 20000);

insert into products
select rownum, 'SKU-' || lpad(rownum, 5, '0'),
       w || ' ' || regexp_substr('белый,чёрный,серый,синий,красный', '[^,]+', 1, 1 + mod(g, 5)) || ' v' || (mod(g, 7) + 1),
       cat, round(dbms_random.value(lo, hi)) - 0.01, trunc(dbms_random.value(0, 500)),
       json_object('гарантия_мес' value 6 * (1 + mod(g, 4)), 'вес_кг' value round(dbms_random.value(0, 10), 2),
                   'страна' value regexp_substr('Китай,Россия,Вьетнам,Корея', '[^,]+', 1, 1 + mod(g, 4)))
from (
  select c.cat, c.lo, c.hi, regexp_substr(c.words, '[^,]+', 1, w.n) w, g.g
  from (select 'Ноутбуки' cat, 'Ноутбук Lumen 14,Ноутбук Pro 16,Ультрабук Air 13,Игровой ноутбук X17' words, 45000 lo, 250000 hi from dual
        union all select 'Телефоны', 'Смартфон Nova,Смартфон Mini,Смартфон Max,Кнопочный телефон', 3000, 140000 from dual
        union all select 'Аксессуары', 'Чехол,Кабель USB-C,Зарядка 65 Вт,Наушники,Мышь,Клавиатура', 300, 15000 from dual
        union all select 'Бытовая техника', 'Чайник,Пылесос,Микроволновка,Кофемашина,Утюг', 1500, 90000 from dual
        union all select 'Мебель', 'Кресло офисное,Стол письменный,Полка,Шкаф', 2500, 60000 from dual) c
  cross join (select level n from dual connect by level <= 6) w
  cross join (select level g from dual connect by level <= 40) g
  where regexp_substr(c.words, '[^,]+', 1, w.n) is not null);

insert /*+ append */ into orders (id, customer_id, employee_id, status, created_at, note)
select g, 1 + trunc(dbms_random.value(0, 20000)), 1 + trunc(dbms_random.value(0, 300)),
       case when r < 0.05 then 'new' when r < 0.12 then 'paid' when r < 0.22 then 'shipped' when r < 0.9 then 'delivered' when r < 0.96 then 'cancelled' else 'returned' end,
       timestamp '2021-01-01 00:00:00' + numtodsinterval(dbms_random.value(0, 1770), 'DAY'),
       case when dbms_random.value < 0.08 then regexp_substr('Позвонить перед доставкой,Подарочная упаковка,Оставить у двери,Срочно!,Счёт на юрлицо', '[^,]+', 1, 1 + trunc(dbms_random.value(0, 5))) end
from (select level g, dbms_random.value r from dual connect by level <= 300000);
commit;

update orders set shipped_at = created_at + numtodsinterval(dbms_random.value(1, 6), 'DAY')
 where status in ('shipped','delivered','returned');

insert /*+ append */ into order_items
select o.id, l.n, p.id, 1 + trunc(dbms_random.value(0, 4)), p.price
from orders o
join (select level n from dual connect by level <= 5) l on l.n <= 1 + mod(o.id, 5)
join products p on p.id = 1 + mod(o.id * 7 + l.n * 131, (select count(*) from products));
commit;

merge into orders o
using (select order_id, sum(qty * price) t from order_items group by order_id) s
on (s.order_id = o.id)
when matched then update set o.total = s.t;

insert into payments (order_id, paid_at, amount, method)
select id, created_at + numtodsinterval(dbms_random.value(0, 2), 'DAY'), total,
       regexp_substr('карта,СБП,наличные,счёт', '[^,]+', 1, 1 + mod(id, 4))
from orders where status not in ('new','cancelled');
commit;

create index orders_customer_ix on orders (customer_id);
create index orders_created_ix on orders (created_at);
create index order_items_product_ix on order_items (product_id);
create index payments_order_ix on payments (order_id);
create index employees_dept_ix on employees (dept_id);
create index customers_city_ix on customers (city, segment);

-- Objects -----------------------------------------------------------------

create or replace view v_order_totals as
select o.id, trunc(o.created_at) as day, c.name as customer, c.city, o.status, o.total,
       (select count(*) from order_items i where i.order_id = o.id) as lines
  from orders o
  join customers c on c.id = o.customer_id;

create or replace view v_staff as
select e.id, e.last_name || ' ' || e.first_name as full_name, d.name as department,
       m.last_name || ' ' || m.first_name as manager, e.salary
  from employees e
  join departments d on d.id = e.dept_id
  left join employees m on m.id = e.manager_id
 where e.active = 'Y';

create materialized view mv_daily_revenue
build immediate refresh complete on demand as
select trunc(created_at) as day, count(*) as orders, sum(total) as revenue
  from orders
 where status not in ('cancelled','returned')
 group by trunc(created_at);

create or replace type money_t as object (
    amount    number(12,2),
    currency  varchar2(3),
    member function show return varchar2
);
/
create or replace type body money_t as
    member function show return varchar2 is
    begin
        return to_char(amount, 'FM999G999G990D00') || ' ' || currency;
    end;
end;
/

create or replace package shop_pkg as
    -- Lifetime value of a customer.
    function customer_ltv(p_customer number) return number;
    -- Cancels orders left new for longer than p_days.
    procedure close_stale_orders(p_days number default 30);
    procedure refresh_revenue;
end shop_pkg;
/
create or replace package body shop_pkg as
    function customer_ltv(p_customer number) return number is
        v number;
    begin
        select nvl(sum(total), 0) into v from orders
         where customer_id = p_customer and status not in ('cancelled','returned');
        return v;
    end;

    procedure close_stale_orders(p_days number default 30) is
    begin
        update orders set status = 'cancelled'
         where status = 'new' and created_at < systimestamp - numtodsinterval(p_days, 'DAY');
        dbms_output.put_line('Отменено заказов: ' || sql%rowcount);
    end;

    procedure refresh_revenue is
    begin
        dbms_mview.refresh('MV_DAILY_REVENUE', 'C');
    end;
end shop_pkg;
/

create or replace function order_vat(p_order number) return number is
    v number;
begin
    select round(total * 20 / 120, 2) into v from orders where id = p_order;
    return v;
end;
/

create or replace trigger orders_status_audit
after update of status on orders
for each row
when (old.status <> new.status)
begin
    insert into order_audit (order_id, old_status, new_status) values (:old.id, :old.status, :new.status);
end;
/

create or replace synonym staff for v_staff;

begin
    dbms_scheduler.create_job(
        job_name        => 'REFRESH_REVENUE_JOB',
        job_type        => 'STORED_PROCEDURE',
        job_action      => 'SHOP_PKG.REFRESH_REVENUE',
        repeat_interval => 'FREQ=HOURLY',
        enabled         => true,
        comments        => 'Пересчёт выручки по дням');
end;
/

exec dbms_stats.gather_schema_stats(user)
exit
