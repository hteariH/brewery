package com.example.petshop;

import java.util.List;
import org.springframework.stereotype.Service;

@Service
public class OwnerService {
    private final OwnerRepository repository;

    public OwnerService(OwnerRepository repository) {
        this.repository = repository;
    }

    List<String> names() {
        return repository.all();
    }

    String name(long id) {
        return repository.all().get((int) id);
    }

    String add(String name) {
        repository.all().add(name);
        return name;
    }
}
