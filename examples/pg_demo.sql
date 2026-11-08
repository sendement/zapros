create table if not exists customers (id serial primary key, name text not null, email text unique, active boolean default true, created_at timestamptz default now());
comment on table customers is 'People who buy';
create table if not exists orders (id serial primary key, customer_id int references customers(id), total numeric(12,2), note text, created_at timestamptz default now());
create index if not exists orders_customer on orders(customer_id);
insert into customers (name, email) select 'Customer ' || g, 'c' || g || '@example.com' from generate_series(1, 500) g on conflict do nothing;
insert into orders (customer_id, total, note) select 1 + (g % 500), round((random() * 2000)::numeric, 2), case when g % 7 = 0 then 'gift' end from generate_series(1, 20000) g;
create or replace view big_orders as select o.*, c.name from orders o join customers c on c.id = o.customer_id where o.total > 1500;
create materialized view if not exists totals as select customer_id, sum(total) s from orders group by 1;
create or replace function vip(p_id int) returns boolean language plpgsql as $$
begin
  if (select sum(total) from orders where customer_id = p_id) > 50000 then
    raise notice 'customer % is vip', p_id;
    return true;
  end if;
  return false;
end $$;
create or replace procedure touch_customer(p_id int) language sql as $$ update customers set created_at = now() where id = p_id $$;
create type order_state as enum ('new', 'paid', 'shipped');
create sequence if not exists invoice_no start 1000;
