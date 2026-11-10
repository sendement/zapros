-- Shop demo for ClickHouse (as a user with access management):
--   clickhouse-client --multiquery < examples/seed/ch.sql
DROP DATABASE IF EXISTS shop;
CREATE DATABASE shop COMMENT 'Демо-магазин';

CREATE TABLE shop.customers
(
    id UInt32,
    name String,
    email Nullable(String),
    city LowCardinality(String),
    segment Enum8('retail' = 1, 'wholesale' = 2, 'vip' = 3),
    created_at DateTime
)
ENGINE = MergeTree ORDER BY id
COMMENT 'Покупатели';

CREATE TABLE shop.products
(
    id UInt32,
    sku String,
    name String,
    category LowCardinality(String),
    price Decimal(10, 2),
    attrs Map(String, String)
)
ENGINE = MergeTree ORDER BY id;

CREATE TABLE shop.orders
(
    id UInt64,
    customer_id UInt32,
    status LowCardinality(String),
    created_at DateTime,
    total Decimal(12, 2),
    items Nested(product_id UInt32, qty UInt16, price Decimal(10, 2))
)
ENGINE = MergeTree
PARTITION BY toYYYYMM(created_at)
ORDER BY (created_at, id);

CREATE TABLE shop.page_views
(
    ts DateTime64(3),
    session_id UUID,
    customer_id Nullable(UInt32),
    url String,
    referrer LowCardinality(String),
    device LowCardinality(String),
    duration_ms UInt32
)
ENGINE = MergeTree
PARTITION BY toYYYYMM(ts)
ORDER BY (ts, session_id)
TTL toDateTime(ts) + INTERVAL 5 YEAR;

INSERT INTO shop.customers
SELECT number + 1,
       concat(['ООО «Ромашка»','ИП Сидоров','АО «Вектор»','ООО «Север»','Петрова Анна','Смирнов Илья','Кузнецова Дарья','Орлов Глеб'][1 + rand(1) % 8], ' #', toString(number + 1)),
       if(rand(2) % 10 = 0, NULL, concat('client', toString(number + 1), '@mail.example')),
       ['Москва','Санкт-Петербург','Казань','Новосибирск','Екатеринбург','Нижний Новгород','Самара','Краснодар'][1 + rand(3) % 8],
       multiIf(rand(4) % 100 < 80, 'retail', rand(4) % 100 < 97, 'wholesale', 'vip'),
       toDateTime('2019-01-01 00:00:00') + rand(5) % (86400 * 2400)
FROM numbers(20000);

INSERT INTO shop.products
SELECT number + 1,
       concat('SKU-', leftPad(toString(number + 1), 5, '0')),
       concat(['Ноутбук Lumen 14','Смартфон Nova','Чехол','Кабель USB-C','Наушники','Чайник','Пылесос','Кресло офисное'][1 + number % 8], ' v', toString(number % 7 + 1)),
       ['Ноутбуки','Телефоны','Аксессуары','Аксессуары','Аксессуары','Бытовая техника','Бытовая техника','Мебель'][1 + number % 8],
       toDecimal64(300 + rand(6) % 150000, 2) - 0.01,
       map('гарантия_мес', toString(6 * (1 + number % 4)), 'страна', ['Китай','Россия','Вьетнам','Корея'][1 + number % 4])
FROM numbers(1000);

INSERT INTO shop.orders
SELECT number + 1 AS id,
       1 + rand(7) % 20000,
       multiIf(rand(8) % 100 < 5, 'new', rand(8) % 100 < 12, 'paid', rand(8) % 100 < 22, 'shipped', rand(8) % 100 < 90, 'delivered', rand(8) % 100 < 96, 'cancelled', 'returned'),
       toDateTime('2021-01-01 00:00:00') + rand(9) % (86400 * 1770),
       arraySum(arrayMap((q, p) -> q * p, qs, ps)),
       arrayMap(i -> toUInt32(1 + (id * 7 + i * 131) % 1000), range(n)),
       qs, ps
FROM
(
    SELECT number, 1 + number % 5 AS n,
           arrayMap(i -> toUInt16(1 + rand(i + number) % 4), range(1 + number % 5)) AS qs,
           arrayMap(i -> toDecimal64(300 + (number * 13 + i * 977) % 150000, 2), range(1 + number % 5)) AS ps
    FROM numbers(1000000)
);

INSERT INTO shop.page_views
SELECT toDateTime64('2024-01-01 00:00:00', 3) + (rand(10) % (86400 * 600)) + (rand(11) % 1000) / 1000,
       generateUUIDv4(number % 400000),
       if(rand(12) % 3 = 0, NULL, 1 + rand(13) % 20000),
       concat('/', ['catalog','product','cart','checkout','search','account'][1 + rand(14) % 6], '/', toString(rand(15) % 1000)),
       ['', 'yandex.ru', 'google.com', 'vk.com', 't.me', 'mail.ru'][1 + rand(16) % 6],
       ['desktop','mobile','tablet'][1 + rand(17) % 3],
       rand(18) % 120000
FROM numbers(5000000);

CREATE TABLE shop.daily_revenue
(
    day Date,
    orders UInt64,
    revenue Decimal(18, 2)
)
ENGINE = SummingMergeTree ORDER BY day;

CREATE MATERIALIZED VIEW shop.daily_revenue_mv TO shop.daily_revenue AS
SELECT toDate(created_at) AS day, count() AS orders, sum(total) AS revenue
FROM shop.orders
WHERE status NOT IN ('cancelled', 'returned')
GROUP BY day;

INSERT INTO shop.daily_revenue
SELECT toDate(created_at), count(), sum(total) FROM shop.orders
WHERE status NOT IN ('cancelled', 'returned') GROUP BY 1;

CREATE VIEW shop.v_top_customers AS
SELECT c.id, c.name, c.city, count() AS orders, sum(o.total) AS revenue
FROM shop.orders AS o
INNER JOIN shop.customers AS c ON c.id = o.customer_id
GROUP BY c.id, c.name, c.city
ORDER BY revenue DESC
LIMIT 100;

CREATE DICTIONARY shop.products_dict
(
    id UInt64,
    name String,
    category String
)
PRIMARY KEY id
SOURCE(CLICKHOUSE(TABLE 'products' DB 'shop' USER 'zapros' PASSWORD 'zapros'))
LIFETIME(MIN 300 MAX 600)
LAYOUT(FLAT());

CREATE FUNCTION IF NOT EXISTS vat AS (total) -> round(total * 20 / 120, 2);

CREATE USER IF NOT EXISTS shop IDENTIFIED WITH sha256_password BY 'shop';
GRANT SELECT, INSERT, ALTER, CREATE TABLE, CREATE VIEW, CREATE DICTIONARY, DROP TABLE, DROP VIEW, TRUNCATE, OPTIMIZE, dictGet ON shop.* TO shop;
GRANT SELECT ON system.* TO shop;
GRANT KILL QUERY ON *.* TO shop;
