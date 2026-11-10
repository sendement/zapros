-- As SYSTEM in the PDB: the shop user.
whenever sqlerror continue
drop user shop cascade;
whenever sqlerror exit failure
create user shop identified by shop default tablespace users quota unlimited on users;
grant create session, create table, create view, create sequence, create procedure,
      create trigger, create type, create synonym, create materialized view, create job to shop;
exit
